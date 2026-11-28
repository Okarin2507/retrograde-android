package dev.okarin.rustygrade.ui

import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import dev.okarin.rustygrade.LibraryViewModel
import dev.okarin.rustygrade.storage.AppPermissions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** Scan and add-folder entry points that ask for what they need first. */
class ScanActions(val scan: () -> Unit, val addFolder: () -> Unit)

/**
 * Wires the permission launcher and the SAF folder picker. On the first launch every permission is
 * requested once; afterwards [ScanActions.scan] asks again on demand and sends the user to the app
 * settings when the system no longer shows the dialog.
 */
@Composable
fun rememberScanActions(viewModel: LibraryViewModel, onFolderPicked: (Uri) -> Unit): ScanActions {
    val context = LocalContext.current
    var requestedByUser by rememberSaveable { mutableStateOf(false) }

    val permissions = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        if (AppPermissions.hasAudioAccess(context)) {
            val empty = viewModel.library.value?.trackCount?.let { it == 0u } != false
            if (empty) viewModel.rescan()
        } else if (requestedByUser) {
            viewModel.reportError("Audio permission denied: enable it in Settings")
            context.startActivity(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null)),
            )
        }
        requestedByUser = false
    }
    val pickedCallback by rememberUpdatedState(onFolderPicked)
    val folder = rememberLauncherForActivityResult(WritableTreePicker()) { uri ->
        if (uri != null) pickedCallback(uri)
    }

    // First launch: ask once, up front, for every permission.
    LaunchedEffect(Unit) {
        val missing = withContext(Dispatchers.IO) {
            AppPermissions.missing(context).takeIf { it.isNotEmpty() && !AppPermissions.askedBefore(context) }
                ?.also { AppPermissions.markAsked(context) }
        }
        if (missing != null) permissions.launch(missing.toTypedArray())
    }

    // Stable instance: a new one per recomposition would rebuild UiEnv (a static composition local)
    // and recompose the whole app on every accent-animation frame.
    return remember(viewModel, context, permissions, folder) {
        ScanActions(
            scan = {
                if (AppPermissions.hasAudioAccess(context)) {
                    viewModel.rescan()
                } else {
                    requestedByUser = true
                    permissions.launch(AppPermissions.missing(context).toTypedArray())
                }
            },
            addFolder = { folder.launch(null) },
        )
    }
}
