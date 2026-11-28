package dev.okarin.rustygrade.ui.components

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.input.InputTransformation
import androidx.compose.foundation.text.input.TextFieldDecorator
import androidx.compose.foundation.text.input.TextFieldLineLimits
import androidx.compose.foundation.text.input.setTextAndPlaceCursorAtEnd
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space

private val GoldCursor = SolidColor(Orbit.Gold)

/** Dark, square single-line text field; the border turns gold while focused. */
@Composable
fun OrbitTextField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    placeholder: String = "",
    keyboardType: KeyboardType = KeyboardType.Text,
    inputTransformation: InputTransformation? = null,
) {
    val source = remember { MutableInteractionSource() }
    val focused by source.collectIsFocusedAsState()
    val state = rememberSyncedTextState(value, onValueChange)
    BasicTextField(
        state = state,
        modifier = modifier,
        lineLimits = TextFieldLineLimits.SingleLine,
        inputTransformation = inputTransformation,
        interactionSource = source,
        textStyle = OrbitType.FieldText,
        cursorBrush = GoldCursor,
        keyboardOptions = KeyboardOptions(keyboardType = keyboardType),
        decorator = TextFieldDecorator { inner ->
            Box(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = Dimens.FieldHeight)
                    .background(Orbit.Void)
                    .border(BorderStroke(Dimens.Stroke, if (focused) Orbit.Gold else Orbit.Line))
                    .padding(horizontal = Space.M),
                contentAlignment = Alignment.CenterStart,
            ) {
                if (state.text.isEmpty() && placeholder.isNotEmpty()) {
                    Text(
                        placeholder,
                        color = Orbit.TextMute,
                        style = OrbitType.Numeric,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                inner()
            }
        },
    )
}

/** Multi-line editable text box (used for synced/plain lyrics in the Tag Editor). */
@Composable
fun OrbitMultilineTextField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    placeholder: String = "",
) {
    val source = remember { MutableInteractionSource() }
    val focused by source.collectIsFocusedAsState()
    val state = rememberSyncedTextState(value, onValueChange)
    BasicTextField(
        state = state,
        modifier = modifier,
        lineLimits = TextFieldLineLimits.MultiLine(minHeightInLines = 5, maxHeightInLines = 10),
        interactionSource = source,
        textStyle = OrbitType.FieldCode,
        cursorBrush = GoldCursor,
        decorator = TextFieldDecorator { inner ->
            Box(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 120.dp)
                    .background(Orbit.Void)
                    .border(BorderStroke(Dimens.Stroke, if (focused) Orbit.Gold else Orbit.Line))
                    .padding(Space.M),
                contentAlignment = Alignment.TopStart,
            ) {
                if (state.text.isEmpty() && placeholder.isNotEmpty()) {
                    Text(
                        placeholder,
                        color = Orbit.TextMute,
                        style = OrbitType.Numeric,
                    )
                }
                inner()
            }
        },
    )
}

/**
 * Tells an echo of what the text field itself reported apart from a genuine external change.
 *
 * The field reports every edit through [onEmit]; the parent echoes each one back as the `value`
 * some recomposition later. [isExternal] is asked for every incoming `value`: an echo is consumed
 * (together with the older echoes it overtook) and ignored, so an echo of an older keystroke never
 * overwrites what has been typed since. Anything else was changed from outside (e.g. a Clear
 * button) and must be written to the field, even when it equals something typed earlier.
 */
internal class EchoFilter(private val capacity: Int = 16) {
    private val emitted = ArrayDeque<String>()

    fun onEmit(text: String) {
        emitted.addLast(text)
        if (emitted.size > capacity) emitted.removeFirst()
    }

    /** `true` when [value] must be written to the field; [current] is what the field shows now. */
    fun isExternal(value: String, current: String): Boolean {
        val index = emitted.indexOf(value)
        if (index >= 0) {
            repeat(index + 1) { emitted.removeFirst() }
            return false
        }
        if (value == current) return false
        // A genuine external change supersedes whatever echoes were still pending.
        emitted.clear()
        return true
    }
}

/**
 * A `TextFieldState` that mirrors a plain `value` / `onValueChange` pair, so callers keep a simple
 * String API while the field uses the state-based `BasicTextField`.
 */
@Composable
fun rememberSyncedTextState(value: String, onValueChange: (String) -> Unit): androidx.compose.foundation.text.input.TextFieldState {
    val state = androidx.compose.foundation.text.input.rememberTextFieldState(value)
    val latestChange by androidx.compose.runtime.rememberUpdatedState(onValueChange)
    val latestValue by androidx.compose.runtime.rememberUpdatedState(value)
    val echoes = remember { EchoFilter() }
    androidx.compose.runtime.LaunchedEffect(value) {
        if (echoes.isExternal(value, state.text.toString())) {
            state.setTextAndPlaceCursorAtEnd(value) // a change that came from outside (e.g. a reset)
        }
    }
    androidx.compose.runtime.LaunchedEffect(state) {
        androidx.compose.runtime.snapshotFlow { state.text.toString() }.collect {
            if (it != latestValue) {
                echoes.onEmit(it)
                latestChange(it)
            }
        }
    }
    return state
}
