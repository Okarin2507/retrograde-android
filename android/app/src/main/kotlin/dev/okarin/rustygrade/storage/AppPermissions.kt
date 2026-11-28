package dev.okarin.rustygrade.storage

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.content.ContextCompat

/** Runtime permissions the app asks for together on first launch. */
object AppPermissions {
    private const val PREFS = "permissions"
    private const val KEY_ASKED = "asked_on_first_launch"

    /**
     * API 33+: audio + images (covers) + notifications. Before that the single legacy storage
     * permission covers both audio and images.
     */
    fun required(): Array<String> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            arrayOf(
                Manifest.permission.READ_MEDIA_AUDIO,
                Manifest.permission.READ_MEDIA_IMAGES,
                Manifest.permission.POST_NOTIFICATIONS,
            )
        } else if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            // API 28-29 edit tags by writing the file, which needs the write permission as well.
            arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE, Manifest.permission.WRITE_EXTERNAL_STORAGE)
        } else {
            arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE)
        }

    fun missing(context: Context): List<String> = required().filter {
        ContextCompat.checkSelfPermission(context, it) != PackageManager.PERMISSION_GRANTED
    }

    /** Reading the music is what the library needs; notifications are optional. */
    fun hasAudioAccess(context: Context): Boolean = MediaStoreSource.hasPermission(context)

    /** The first-launch prompt is shown once; afterwards the SCAN button asks again on demand. */
    fun askedBefore(context: Context): Boolean =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_ASKED, false)

    fun markAsked(context: Context) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_ASKED, true).apply()
    }
}
