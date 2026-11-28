package dev.okarin.rustygrade.ui

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.PowerManager
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat

/** Battery saver state, kept current by the system broadcast while this is composed. */
@Composable
fun rememberPowerSaveMode(): Boolean {
    val context = LocalContext.current
    val pm = remember(context) { context.getSystemService(Context.POWER_SERVICE) as PowerManager }
    var saving by remember { mutableStateOf(pm.isPowerSaveMode) }
    DisposableEffect(context, pm) {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                saving = pm.isPowerSaveMode
            }
        }
        ContextCompat.registerReceiver(
            context,
            receiver,
            IntentFilter(PowerManager.ACTION_POWER_SAVE_MODE_CHANGED),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        saving = pm.isPowerSaveMode
        onDispose { context.unregisterReceiver(receiver) }
    }
    return saving
}

/**
 * Battery saver state, provided once by `OrbitApp` so a single broadcast receiver serves the
 * mini-bar and the player backdrop. Defaults to `false` outside the app shell.
 */
val LocalPowerSaveMode = compositionLocalOf { false }
