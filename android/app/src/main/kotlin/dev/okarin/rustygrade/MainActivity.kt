package dev.okarin.rustygrade

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import dev.okarin.rustygrade.ui.OrbitApp

class MainActivity : ComponentActivity() {
    private val library: LibraryViewModel by viewModels()
    private val collections: CollectionsViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(Color.Transparent.toArgb()),
            navigationBarStyle = SystemBarStyle.dark(Color.Transparent.toArgb()),
        )
        val engine = (application as RustygradeApp).engine
        setContent {
            OrbitApp(
                engine = engine,
                library = library,
                collections = collections,
                onFolderPicked = { uri ->
                    try {
                        // Without a persisted grant the folder is unreadable after the process dies.
                        try {
                            contentResolver.takePersistableUriPermission(
                                uri,
                                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                            )
                        } catch (_: SecurityException) {
                            // Some providers only offer read access: scanning and playing still work.
                            contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
                        }
                        library.addTreeAndRescan(uri.toString())
                    } catch (e: SecurityException) {
                        library.reportError("This folder cannot be remembered: ${e.message}")
                    }
                },
            )
        }
    }
}
