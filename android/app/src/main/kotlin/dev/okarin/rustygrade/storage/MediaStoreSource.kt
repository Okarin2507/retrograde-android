package dev.okarin.rustygrade.storage

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.provider.MediaStore
import androidx.core.content.ContextCompat

/** Audio files that `MediaStore` indexes on shared storage, as real filesystem paths. */
object MediaStoreSource {
    class Result(
        /** `MediaStore.Audio.Media.DATA` paths of music files. */
        val files: List<String>,
        /** Volume roots (`/storage/emulated/0`, `/storage/XXXX-XXXX`): authoritative for "missing" detection. */
        val roots: List<String>,
    )

    /** `READ_MEDIA_AUDIO` on API 33+, `READ_EXTERNAL_STORAGE` before. */
    fun requiredPermission(): String =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            Manifest.permission.READ_MEDIA_AUDIO
        } else {
            Manifest.permission.READ_EXTERNAL_STORAGE
        }

    fun hasPermission(context: Context): Boolean =
        ContextCompat.checkSelfPermission(context, requiredPermission()) ==
            PackageManager.PERMISSION_GRANTED

    /** Returns null when the permission is missing or the query failed (never report "everything is gone"). */
    fun query(context: Context): Result? {
        if (!hasPermission(context)) return null
        val files = ArrayList<String>()
        try {
            context.contentResolver.query(
                MediaStore.Audio.Media.EXTERNAL_CONTENT_URI,
                arrayOf(MediaStore.Audio.Media.DATA),
                // Skips ringtones, alarms, notifications, recordings and podcasts.
                "${MediaStore.Audio.Media.IS_MUSIC} != 0",
                null,
                null,
            )?.use { cursor ->
                val col = cursor.getColumnIndexOrThrow(MediaStore.Audio.Media.DATA)
                while (cursor.moveToNext()) {
                    cursor.getString(col)?.takeIf { it.isNotEmpty() }?.let(files::add)
                }
            } ?: return null
        } catch (_: Exception) {
            return null
        }
        return Result(files, volumeRoots(context))
    }

    private fun volumeRoots(context: Context): List<String> =
        context.getExternalFilesDirs(null)
            .filterNotNull()
            .mapNotNull { dir ->
                val path = dir.absolutePath
                val idx = path.indexOf("/Android/")
                if (idx > 0) path.substring(0, idx) else null
            }
            .distinct()
}
