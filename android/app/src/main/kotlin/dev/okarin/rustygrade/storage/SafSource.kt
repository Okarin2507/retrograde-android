package dev.okarin.rustygrade.storage

import android.content.ContentResolver
import android.content.Context
import android.net.Uri
import android.provider.DocumentsContract
import uniffi.rustygrade_ffi.SafDocument
import uniffi.rustygrade_ffi.UriOpener

/** Music folders picked with `ACTION_OPEN_DOCUMENT_TREE`, walked with fast child-document queries. */
object SafSource {
    class Tree(val rootDocumentId: String, val documents: List<SafDocument>)

    /** True while the app still holds the persisted read grant for [treeUri]. */
    fun hasPersistedGrant(context: Context, treeUri: Uri): Boolean =
        context.contentResolver.persistedUriPermissions.any { it.uri == treeUri && it.isReadPermission }

    /**
     * Lists every audio document below [treeUri]. Returns null when any directory cannot be read
     * (revoked grant, unmounted card): the caller then leaves that tree out of the scan so its
     * tracks are not mistaken for deleted ones. [shouldContinue] is polled between directories: when
     * it turns false (the scan was cancelled) the walk stops at once and also returns null.
     */
    fun walk(context: Context, treeUri: Uri, shouldContinue: () -> Boolean = { true }): Tree? {
        if (!hasPersistedGrant(context, treeUri)) return null
        val resolver = context.contentResolver
        val rootId = try {
            DocumentsContract.getTreeDocumentId(treeUri)
        } catch (_: Exception) {
            return null
        }
        val docs = ArrayList<SafDocument>()
        // Each pending directory carries its logical path (see SafPaths) for the files below it.
        val pending = ArrayDeque<Pair<String, String>>()
        val visited = HashSet<String>() // providers with directory cycles must not loop forever
        pending.add(rootId to rootId)
        while (pending.isNotEmpty()) {
            if (!shouldContinue()) return null
            val (dirId, dirLogical) = pending.removeLast()
            if (!visited.add(dirId)) continue
            val children = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, dirId)
            try {
                resolver.query(children, PROJECTION, null, null, null)?.use { c ->
                    val iId = c.getColumnIndex(DocumentsContract.Document.COLUMN_DOCUMENT_ID)
                    val iMime = c.getColumnIndex(DocumentsContract.Document.COLUMN_MIME_TYPE)
                    val iName = c.getColumnIndex(DocumentsContract.Document.COLUMN_DISPLAY_NAME)
                    val iMtime = c.getColumnIndex(DocumentsContract.Document.COLUMN_LAST_MODIFIED)
                    val iSize = c.getColumnIndex(DocumentsContract.Document.COLUMN_SIZE)
                    if (iId < 0 || iMime < 0) return null
                    while (c.moveToNext()) {
                        if (!shouldContinue()) return null
                        val id = c.getString(iId) ?: continue
                        val mime = c.getString(iMime).orEmpty()
                        val name = if (iName >= 0) c.getString(iName).orEmpty() else ""
                        if (mime == DocumentsContract.Document.MIME_TYPE_DIR) {
                            pending.add(id to SafPaths.logicalDir(dirLogical, name))
                        } else if (isAudio(mime, name)) {
                            docs.add(
                                SafDocument(
                                    // The logical path, not the provider's id: Rust identifies tracks by it.
                                    documentId = SafPaths.logicalFile(dirLogical, id, name),
                                    uri = DocumentsContract.buildDocumentUriUsingTree(treeUri, id).toString(),
                                    // Optional columns: 0 when the provider omits them.
                                    mtimeMs = if (iMtime >= 0) c.getLong(iMtime) else 0L,
                                    size = if (iSize >= 0) c.getLong(iSize) else 0L,
                                ),
                            )
                        }
                    }
                } ?: return null
            } catch (_: Exception) {
                return null
            }
        }
        return Tree(rootId, docs)
    }

    private fun isAudio(mime: String, name: String): Boolean =
        mime.startsWith("audio/") || SafPaths.hasAudioExtension(name)

    /** Queries the current `(mtimeMs, size)` for a single SAF document URI, or null if unavailable. */
    fun queryMetadata(resolver: ContentResolver, uri: Uri): Pair<Long, Long>? = try {
        resolver.query(
            uri,
            arrayOf(
                DocumentsContract.Document.COLUMN_LAST_MODIFIED,
                DocumentsContract.Document.COLUMN_SIZE,
            ),
            null,
            null,
            null,
        )?.use { c ->
            if (!c.moveToFirst()) return null
            val iMtime = c.getColumnIndex(DocumentsContract.Document.COLUMN_LAST_MODIFIED)
            val iSize = c.getColumnIndex(DocumentsContract.Document.COLUMN_SIZE)
            val mtime = if (iMtime >= 0 && !c.isNull(iMtime)) c.getLong(iMtime) else 0L
            val size = if (iSize >= 0 && !c.isNull(iSize)) c.getLong(iSize) else 0L
            mtime to size
        }
    } catch (_: Exception) {
        null
    }

    private val PROJECTION = arrayOf(
        DocumentsContract.Document.COLUMN_DOCUMENT_ID,
        DocumentsContract.Document.COLUMN_MIME_TYPE,
        DocumentsContract.Document.COLUMN_DISPLAY_NAME,
        DocumentsContract.Document.COLUMN_LAST_MODIFIED,
        DocumentsContract.Document.COLUMN_SIZE,
    )
}

/**
 * Hands Rust an fd for a `content://` URI. `detachFd()` transfers ownership: Rust closes it, so the
 * `ParcelFileDescriptor` must not be closed here.
 */
class ContentResolverOpener(private val resolver: ContentResolver) : UriOpener {
    override fun openFd(uri: String): Int? = try {
        resolver.openFileDescriptor(Uri.parse(uri), "r")?.detachFd()
    } catch (_: Exception) {
        null
    }
}
