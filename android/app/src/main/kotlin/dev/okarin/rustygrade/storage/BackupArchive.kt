package dev.okarin.rustygrade.storage

import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FilterInputStream
import java.io.InputStream
import java.io.OutputStream
import java.io.RandomAccessFile
import java.nio.charset.StandardCharsets
import java.util.Locale
import java.util.zip.ZipEntry
import java.util.zip.ZipInputStream
import java.util.zip.ZipOutputStream
import uniffi.rustygrade_ffi.BackupBundleDto
import uniffi.rustygrade_ffi.BackupCoverEntry

/**
 * Pure JVM ZIP serializer/deserializer for Rustygrade backups (`native-collections.json`,
 * `native-stats.json`, `native-settings.json`, and `covers/native_pl_*` or `covers/c_<24-hex>.*`
 * playlist covers).
 */
object BackupArchive {
    const val MAX_JSON_BYTES = 16 * 1024 * 1024
    const val MAX_COVER_BYTES = 10 * 1024 * 1024
    const val MAX_TOTAL_BYTES = 64 * 1024 * 1024
    const val MAX_ACCEPTED_ENTRIES = 500
    const val MAX_SCANNED_ENTRIES = 50_000

    /**
     * Ignored entries (a desktop backup also carries its album-cover cache, often hundreds of MB of
     * JPEGs) are decompressed and thrown away. They must not count against [MAX_TOTAL_BYTES], or real
     * archives would be rejected, so they get their own limits: an absolute ceiling, and a
     * decompressed:compressed ratio limit (a zip bomb is highly compressible, a JPEG cache is not).
     */
    const val MAX_IGNORED_BYTES = 1L shl 30
    const val IGNORED_RATIO_GRACE_BYTES = 8L * 1024 * 1024
    const val MAX_IGNORED_RATIO = 100L
    private const val MAX_SPOOL_BYTES = 128L * 1024 * 1024

    private const val FILE_COLLECTIONS = "native-collections.json"
    private const val FILE_STATS = "native-stats.json"
    private const val FILE_SETTINGS = "native-settings.json"
    private val LEGACY_COVER_NAME = Regex("^native_pl_[A-Za-z0-9_.-]{1,120}\\.(?i:jpg|jpeg|png|webp)$")
    private val CONTENT_COVER_NAME = Regex("^c_[0-9a-f]{24}\\.(?i:jpg|jpeg|png|webp)$")
    private val JSON_COVER_ART = Regex("\"coverArt\"\\s*:\\s*\"((?:[^\"\\\\]|\\\\.)*)\"")

    fun writeZip(output: OutputStream, bundle: BackupBundleDto) {
        ZipOutputStream(output).use { zos ->
            bundle.collectionsJson?.let { writeTextEntry(zos, FILE_COLLECTIONS, it) }
            bundle.statsJson?.let { writeTextEntry(zos, FILE_STATS, it) }
            bundle.settingsJson?.let { writeTextEntry(zos, FILE_SETTINGS, it) }
            val seenCovers = HashSet<String>()
            for (cover in bundle.playlistCovers) {
                require(isSafeCoverFilename(cover.name)) { "Invalid playlist cover filename: ${cover.name}" }
                if (seenCovers.add(cover.name)) {
                    zos.putNextEntry(ZipEntry("covers/${cover.name}"))
                    zos.write(cover.bytes)
                    zos.closeEntry()
                }
            }
        }
    }

    /** Reads a backup ZIP, reopening [openStream] for a second pass only when needed. */
    fun readZip(openStream: () -> InputStream): BackupBundleDto =
        openStream().use { firstPass ->
            readZipInternal(firstPass, reopenForSecondPass = openStream)
        }

    /** Reads a backup ZIP starting from an already-opened [firstPass] stream, reopening only if needed. */
    fun readZip(firstPass: InputStream, reopenForSecondPass: () -> InputStream): BackupBundleDto =
        readZipInternal(firstPass, reopenForSecondPass)

    /** Reads a backup ZIP from a single [InputStream] (uses `mark`/`reset` or bounded temp-file spooling). */
    fun readZip(input: InputStream): BackupBundleDto {
        if (!input.markSupported()) {
            return readZipInternal(input, reopenForSecondPass = null)
        }
        input.mark(Int.MAX_VALUE)
        return try {
            readZipInternal(
                NonClosingInputStream(input),
                reopenForSecondPass = {
                    input.reset()
                    NonClosingInputStream(input)
                },
            )
        } finally {
            input.close()
        }
    }

    private class SpooledEntry(val name: String, val offset: Long, val length: Int)

    private fun readZipInternal(
        firstPass: InputStream,
        reopenForSecondPass: (() -> InputStream)?,
    ): BackupBundleDto {
        var collectionsJson: String? = null
        var collectionsDepth = Int.MAX_VALUE
        var statsJson: String? = null
        var statsDepth = Int.MAX_VALUE
        var settingsJson: String? = null
        var settingsDepth = Int.MAX_VALUE

        val covers = ArrayList<BackupCoverEntry>()
        val seenLegacyCovers = HashSet<String>()
        val seenContentCovers = HashSet<String>()
        var referencedContentCovers: Set<String> = emptySet()
        val skippedContentCovers = HashSet<String>()
        val spooled = LinkedHashMap<String, SpooledEntry>()
        var spoolFile: File? = null
        var spoolStream: OutputStream? = null
        var spooledBytes = 0L

        var totalBytes = 0L
        var acceptedEntries = 0
        var ignoredBytes = 0L
        val scratch = ByteArray(SCRATCH_BYTES)

        fun drain(zis: ZipInputStream, counting: CountingInputStream, entryName: String) {
            while (true) {
                val n = zis.read(scratch)
                if (n < 0) break
                if (n == 0) continue
                ignoredBytes += n
                require(ignoredBytes <= MAX_IGNORED_BYTES) {
                    "Backup archive holds too much unused data (entry '$entryName')"
                }
                require(ignoredBytes <= IGNORED_RATIO_GRACE_BYTES || ignoredBytes <= MAX_IGNORED_RATIO * counting.count) {
                    "Backup archive looks like a zip bomb (entry '$entryName')"
                }
            }
        }

        fun acceptBudget(entryName: String) {
            acceptedEntries += 1
            require(acceptedEntries <= MAX_ACCEPTED_ENTRIES) {
                "Backup archive has too many accepted backup files (max $MAX_ACCEPTED_ENTRIES)"
            }
            require(totalBytes <= MAX_TOTAL_BYTES) {
                "Archive entry '$entryName' exceeds size limit"
            }
        }

        try {
            // Pass 1: read JSONs, legacy covers, and any content covers already known from depth-1 collections.
            val counting1 = CountingInputStream(firstPass)
            var scanned1 = 0
            ZipInputStream(counting1).use { zis ->
                while (true) {
                    val entry = zis.nextEntry ?: break
                    scanned1 += 1
                    require(scanned1 <= MAX_SCANNED_ENTRIES) {
                        "Backup archive has too many entries (max $MAX_SCANNED_ENTRIES)"
                    }

                    val segs = validateAndSplitEntryPath(entry.name)
                    if (entry.isDirectory || segs == null) {
                        drain(zis, counting1, entry.name)
                        zis.closeEntry()
                        continue
                    }

                    when (val role = classifyEntry(segs)) {
                        is EntryRole.Json -> {
                            acceptBudget(entry.name)
                            val remaining = (MAX_TOTAL_BYTES - totalBytes).coerceAtLeast(0L).toInt()
                            val bytes = readBounded(zis, minOf(MAX_JSON_BYTES, remaining), entry.name)
                            totalBytes += bytes.size
                            val text = String(bytes, StandardCharsets.UTF_8)
                            when (role.file) {
                                FILE_COLLECTIONS -> if (role.depth < collectionsDepth) {
                                    collectionsJson = text
                                    collectionsDepth = role.depth
                                    referencedContentCovers = extractReferencedContentCovers(text)
                                }
                                FILE_STATS -> if (role.depth < statsDepth) {
                                    statsJson = text
                                    statsDepth = role.depth
                                }
                                FILE_SETTINGS -> if (role.depth < settingsDepth) {
                                    settingsJson = text
                                    settingsDepth = role.depth
                                }
                            }
                        }
                        is EntryRole.LegacyCover -> if (role.name in seenLegacyCovers) {
                            drain(zis, counting1, entry.name)
                        } else {
                            acceptBudget(entry.name)
                            val remaining = (MAX_TOTAL_BYTES - totalBytes).coerceAtLeast(0L).toInt()
                            val bytes = readBounded(zis, minOf(MAX_COVER_BYTES, remaining), entry.name)
                            totalBytes += bytes.size
                            seenLegacyCovers.add(role.name)
                            covers.add(BackupCoverEntry(role.name, bytes))
                        }
                        is EntryRole.ContentCover -> {
                            val lower = role.name.lowercase(Locale.ROOT)
                            if (collectionsDepth == 1) {
                                if (lower in referencedContentCovers && lower !in seenContentCovers) {
                                    acceptBudget(entry.name)
                                    val remaining = (MAX_TOTAL_BYTES - totalBytes).coerceAtLeast(0L).toInt()
                                    val bytes = readBounded(zis, minOf(MAX_COVER_BYTES, remaining), entry.name)
                                    totalBytes += bytes.size
                                    seenContentCovers.add(lower)
                                    covers.add(BackupCoverEntry(role.name, bytes))
                                } else {
                                    drain(zis, counting1, entry.name)
                                }
                            } else if (reopenForSecondPass != null) {
                                drain(zis, counting1, entry.name)
                                skippedContentCovers.add(lower)
                            } else if (lower in spooled) {
                                drain(zis, counting1, entry.name)
                            } else {
                                val bytes = readBounded(zis, MAX_COVER_BYTES, entry.name)
                                spooledBytes += bytes.size
                                require(spooledBytes <= MAX_SPOOL_BYTES) {
                                    "Backup archive holds too much cover data before collections index (entry '${entry.name}')"
                                }
                                require(spooledBytes <= IGNORED_RATIO_GRACE_BYTES || spooledBytes <= MAX_IGNORED_RATIO * counting1.count) {
                                    "Backup archive looks like a zip bomb (entry '${entry.name}')"
                                }
                                val out = spoolStream ?: File.createTempFile("rustygrade-zip-", ".spool").let { file ->
                                    spoolFile = file
                                    file.outputStream().also { spoolStream = it }
                                }
                                val offset = spooledBytes - bytes.size
                                out.write(bytes)
                                spooled[lower] = SpooledEntry(role.name, offset, bytes.size)
                            }
                        }
                        EntryRole.Ignore -> drain(zis, counting1, entry.name)
                    }
                    zis.closeEntry()
                }
            }
            spoolStream?.close()
            spoolStream = null

            require(collectionsJson != null || statsJson != null || settingsJson != null) {
                "Backup archive contains no Rustygrade data files"
            }

            val finalReferenced = extractReferencedContentCovers(collectionsJson)
            val missing = finalReferenced.filterTo(HashSet()) { it !in seenContentCovers }

            if (missing.isNotEmpty() && reopenForSecondPass != null && missing.any { it in skippedContentCovers }) {
                reopenForSecondPass().use { secondPass ->
                    val counting2 = CountingInputStream(secondPass)
                    var scanned2 = 0
                    ZipInputStream(counting2).use { zis2 ->
                        while (true) {
                            val entry = zis2.nextEntry ?: break
                            scanned2 += 1
                            require(scanned2 <= MAX_SCANNED_ENTRIES) {
                                "Backup archive has too many entries (max $MAX_SCANNED_ENTRIES)"
                            }
                            val segs = validateAndSplitEntryPath(entry.name)
                            if (entry.isDirectory || segs == null) {
                                drain(zis2, counting2, entry.name)
                                zis2.closeEntry()
                                continue
                            }
                            when (val role = classifyEntry(segs)) {
                                is EntryRole.ContentCover -> {
                                    val lower = role.name.lowercase(Locale.ROOT)
                                    if (lower in missing && seenContentCovers.add(lower)) {
                                        acceptBudget(entry.name)
                                        val remaining = (MAX_TOTAL_BYTES - totalBytes).coerceAtLeast(0L).toInt()
                                        val bytes = readBounded(zis2, minOf(MAX_COVER_BYTES, remaining), entry.name)
                                        totalBytes += bytes.size
                                        covers.add(BackupCoverEntry(role.name, bytes))
                                    } else {
                                        drain(zis2, counting2, entry.name)
                                    }
                                }
                                else -> drain(zis2, counting2, entry.name)
                            }
                            zis2.closeEntry()
                        }
                    }
                }
            } else if (missing.isNotEmpty() && spoolFile != null) {
                RandomAccessFile(spoolFile, "r").use { raf ->
                    for (lower in missing) {
                        val sp = spooled[lower] ?: continue
                        if (!seenContentCovers.add(lower)) continue
                        acceptBudget(sp.name)
                        val remaining = (MAX_TOTAL_BYTES - totalBytes).coerceAtLeast(0L).toInt()
                        require(sp.length <= remaining) { "Archive entry '${sp.name}' exceeds size limit" }
                        val bytes = ByteArray(sp.length)
                        raf.seek(sp.offset)
                        raf.readFully(bytes)
                        totalBytes += bytes.size
                        covers.add(BackupCoverEntry(sp.name, bytes))
                    }
                }
            }
        } finally {
            runCatching { spoolStream?.close() }
            spoolFile?.delete()
        }

        return BackupBundleDto(
            collectionsJson = collectionsJson,
            statsJson = statsJson,
            settingsJson = settingsJson,
            playlistCovers = covers,
        )
    }

    internal fun extractReferencedContentCovers(collectionsJson: String?): Set<String> {
        if (collectionsJson.isNullOrEmpty()) return emptySet()
        val out = HashSet<String>()
        for (match in JSON_COVER_ART.findAll(collectionsJson)) {
            val raw = match.groupValues[1]
                .replace("\\/", "/")
                .replace("\\\\", "\\")
            val noQuery = raw.substringBefore('?')
            val base = noQuery.substringAfterLast('/').substringAfterLast('\\').trim()
            if (CONTENT_COVER_NAME.matches(base)) {
                out.add(base.lowercase(Locale.ROOT))
            }
        }
        return out
    }

    private fun writeTextEntry(zos: ZipOutputStream, name: String, content: String) {
        zos.putNextEntry(ZipEntry(name))
        zos.write(content.toByteArray(StandardCharsets.UTF_8))
        zos.closeEntry()
    }

    private sealed interface EntryRole {
        data class Json(val file: String, val depth: Int) : EntryRole
        data class LegacyCover(val name: String) : EntryRole
        data class ContentCover(val name: String) : EntryRole
        data object Ignore : EntryRole
    }

    private fun classifyEntry(segs: List<String>): EntryRole {
        val jsonName = when {
            segs.size == 1 -> segs[0] to 1
            segs.size == 2 && segs[0] != "covers" -> segs[1] to 2
            else -> null
        }
        if (jsonName != null) {
            val (name, depth) = jsonName
            if (name == FILE_COLLECTIONS || name == FILE_STATS || name == FILE_SETTINGS) {
                return EntryRole.Json(name, depth)
            }
        }

        val coverName = when {
            segs.size == 2 && segs[0] == "covers" -> segs[1]
            segs.size == 3 && segs[1] == "covers" -> segs[2]
            else -> null
        }
        if (coverName != null && isSafeCoverCharacters(coverName)) {
            if (LEGACY_COVER_NAME.matches(coverName)) {
                return EntryRole.LegacyCover(coverName)
            }
            if (CONTENT_COVER_NAME.matches(coverName)) {
                return EntryRole.ContentCover(coverName)
            }
        }
        return EntryRole.Ignore
    }

    private fun validateAndSplitEntryPath(rawName: String): List<String>? {
        require('\u0000' !in rawName) { "Unsafe archive entry name" }
        require(!rawName.startsWith('/') && !rawName.startsWith('\\')) {
            "Absolute path not allowed in archive: $rawName"
        }
        require(!(rawName.length >= 2 && rawName[0].isLetter() && rawName[1] == ':')) {
            "Drive path not allowed in archive: $rawName"
        }
        val normalized = rawName.replace('\\', '/')
        val rawSegments = normalized.split('/')
        require(rawSegments.none { it == ".." }) { "Path traversal not allowed in archive: $rawName" }
        val segs = rawSegments.filter { it.isNotEmpty() && it != "." }
        if (segs.isEmpty() || segs.first() == "__MACOSX") return null
        return segs
    }

    private fun isSafeCoverCharacters(name: String): Boolean =
        !(name.isEmpty() || '/' in name || '\\' in name || '\u0000' in name || ".." in name)

    internal fun isSafeCoverFilename(name: String): Boolean =
        isSafeCoverCharacters(name) && (LEGACY_COVER_NAME.matches(name) || CONTENT_COVER_NAME.matches(name))

    private class NonClosingInputStream(input: InputStream) : FilterInputStream(input) {
        override fun close() = Unit
    }

    /** Counts the compressed bytes pulled from the archive (the denominator of the bomb ratio). */
    private class CountingInputStream(input: InputStream) : FilterInputStream(input) {
        var count = 0L
            private set

        override fun read(): Int = super.read().also { if (it >= 0) count++ }

        override fun read(b: ByteArray, off: Int, len: Int): Int =
            super.read(b, off, len).also { if (it > 0) count += it }

        override fun skip(n: Long): Long = super.skip(n).also { if (it > 0) count += it }
    }

    private const val SCRATCH_BYTES = 8192

    private fun readBounded(input: InputStream, maxBytes: Int, entryName: String): ByteArray {
        val out = ByteArrayOutputStream()
        val buf = ByteArray(SCRATCH_BYTES)
        var total = 0
        while (true) {
            val n = input.read(buf)
            if (n <= 0) break
            total += n
            require(total <= maxBytes) { "Archive entry '$entryName' exceeds size limit" }
            out.write(buf, 0, n)
        }
        return out.toByteArray()
    }
}
