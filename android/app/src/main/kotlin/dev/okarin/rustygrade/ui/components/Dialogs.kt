package dev.okarin.rustygrade.ui.components

import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.RectangleShape
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType

/** The `WARNING` confirmation used before destructive actions. */
@Composable
fun ConfirmDialog(
    message: String,
    confirmLabel: String,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
    title: String = "WARNING",
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        shape = RectangleShape,
        containerColor = Orbit.Raised,
        title = {
            Text(
                title.uppercase(),
                color = Orbit.Danger,
                style = OrbitType.DialogTitle,
            )
        },
        text = { Text(message, color = Orbit.Text, style = OrbitType.Input) },
        confirmButton = {
            TextButton(onClick = onConfirm) {
                Text(confirmLabel, color = Orbit.Danger, style = OrbitType.Button)
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("CANCEL", color = Orbit.TextDim, style = OrbitType.Button) }
        },
    )
}
