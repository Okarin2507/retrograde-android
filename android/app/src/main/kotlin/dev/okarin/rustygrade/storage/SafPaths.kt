package dev.okarin.rustygrade.storage

/**
 * Path-shaped identity for SAF documents (the Rust scanner groups albums and detects missing files
 * from it, and filters audio by its extension).
 */
object SafPaths {
    private val AUDIO_EXTENSIONS =
        setOf("flac", "mp3", "wav", "aac", "ogg", "m4a", "wma", "aiff", "alac", "opus")

    fun hasAudioExtension(name: String): Boolean =
        name.substringAfterLast('.', "").lowercase() in AUDIO_EXTENSIONS

    /** Joins two path pieces; a volume root such as `primary:` takes no extra slash. */
    fun join(parent: String, name: String): String =
        if (parent.endsWith(":") || parent.endsWith("/")) parent + name else "$parent/$name"

    /**
     * Logical path of a directory entry. Providers like ExternalStorage already use readable ids
     * (`primary:Music/Album/01.flac`): those are kept unchanged, so favorites, playlists and stats
     * keyed by the old id still match. Providers with opaque ids (numbers, hashes) get a path built
     * from the display names instead, which is what makes their files recognisable as audio.
     */
    fun logicalFile(parentLogical: String, documentId: String, displayName: String): String =
        if (hasAudioExtension(documentId.substringAfterLast('/').substringAfterLast(':'))) {
            documentId
        } else {
            join(parentLogical, displayName)
        }

    /** Logical path of a sub-directory: always built from names, so children can build on it. */
    fun logicalDir(parentLogical: String, displayName: String): String = join(parentLogical, displayName)
}
