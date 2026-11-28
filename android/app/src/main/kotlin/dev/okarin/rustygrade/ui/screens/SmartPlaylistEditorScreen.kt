package dev.okarin.rustygrade.ui.screens

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.ConfirmDialog
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.EmptyMessage
import dev.okarin.rustygrade.ui.components.OrbitButton
import dev.okarin.rustygrade.ui.components.OrbitChip
import dev.okarin.rustygrade.ui.components.OrbitTextField
import dev.okarin.rustygrade.ui.components.ScreenHeader
import dev.okarin.rustygrade.ui.components.SectionTitle
import dev.okarin.rustygrade.ui.components.formatDuration
import dev.okarin.rustygrade.ui.components.pluralCount
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.delay
import uniffi.rustygrade_ffi.SmartFieldInfo
import uniffi.rustygrade_ffi.SmartPlaylistInfo
import uniffi.rustygrade_ffi.SmartPreview
import uniffi.rustygrade_ffi.SmartRuleInfo
import uniffi.rustygrade_ffi.SortOption
import uniffi.rustygrade_ffi.smartFields
import uniffi.rustygrade_ffi.smartSorts

/** Fields whose value is a number (digits keyboard). */
private val NumericFields = setOf("bits", "rate", "year", "duration", "plays", "played", "added")

private const val PreviewRows = 10

private fun blankDraft() = SmartPlaylistInfo(
    id = 0L, name = "", matchAll = true, rules = emptyList(), sort = "album", limit = 0u,
)

/**
 * Create (`id == 0`) or edit a smart playlist: name, MATCH ALL/ANY, rule cards, sort and limit,
 * with a live preview of the matching tracks.
 */
@Composable
fun SmartPlaylistEditorScreen(id: Long) {
    val env = LocalEnv.current
    val snapshot by env.collections.collections.collectAsStateWithLifecycle()
    val fields by produceState(emptyList<SmartFieldInfo>()) { value = io { smartFields() } }
    val sorts by produceState(emptyList<SortOption>()) { value = io { smartSorts() } }
    val sortOptions = remember(sorts) { sorts.map { it.key to it.label } }

    // Saveable: rotating the device must not throw away unsaved edits.
    var saving by rememberSaveable(id) { mutableStateOf(false) }
    var draft by rememberSaveable(id, stateSaver = SmartDraftSaver) {
        mutableStateOf<SmartPlaylistInfo?>(if (id == 0L) blankDraft() else null)
    }
    LaunchedEffect(id, snapshot) {
        if (draft == null) draft = snapshot?.smart?.firstOrNull { it.id == id }?.copy()
    }
    val current = draft
    val missing = current == null && snapshot != null

    var preview by remember { mutableStateOf<SmartPreview?>(null) }
    var previewFailed by remember { mutableStateOf(false) }
    // The name does not affect the matches, so leave it out of the key.
    val previewKey = current?.copy(name = "")
    LaunchedEffect(previewKey) {
        val d = previewKey ?: return@LaunchedEffect
        delay(250)
        try {
            preview = io { env.engine.smartPreview(d) }
            previewFailed = false
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            previewFailed = true
        }
    }

    var confirmDelete by rememberSaveable { mutableStateOf(false) }

    Column(Modifier.fillMaxSize()) {
        ScreenHeader(if (id == 0L) "NEW SMART PLAYLIST" else "EDIT SMART PLAYLIST", onBack = { env.nav.pop() })
        when {
            missing -> EmptyMessage("PLAYLIST NOT FOUND")
            current == null -> EmptyMessage("LOADING…")
            else -> Column(
                Modifier.fillMaxSize().imePadding().verticalScroll(rememberScrollState()).padding(horizontal = Space.L, vertical = Dimens.RowPadV),
                verticalArrangement = Arrangement.spacedBy(Space.M),
            ) {
                SectionTitle("NAME")
                OrbitTextField(
                    current.name, { draft = current.copy(name = it) },
                    Modifier.fillMaxWidth(), placeholder = "SMART PLAYLIST NAME",
                )

                SectionTitle("MATCH", Modifier.padding(top = Space.Compact))
                Row(horizontalArrangement = Arrangement.spacedBy(Space.S), verticalAlignment = Alignment.CenterVertically) {
                    OrbitChip("ALL", current.matchAll, { draft = current.copy(matchAll = true) })
                    OrbitChip("ANY", !current.matchAll, { draft = current.copy(matchAll = false) })
                    Text(
                        "OF THE RULES BELOW",
                        color = Orbit.TextDim,
                        style = OrbitType.Status,
                        modifier = Modifier.padding(start = Space.Xs),
                    )
                }

                SectionTitle("RULES", Modifier.padding(top = Space.Compact))
                if (current.rules.isEmpty()) {
                    Text(
                        "No rules yet — every track in your library matches.",
                        color = Orbit.TextDim,
                        style = OrbitType.Body,
                    )
                }
                current.rules.forEachIndexed { index, rule ->
                    RuleCard(
                        rule = rule,
                        fields = fields,
                        onChange = { updated ->
                            draft = current.copy(rules = current.rules.mapIndexed { i, r -> if (i == index) updated else r })
                        },
                        onRemove = { draft = current.copy(rules = current.rules.filterIndexed { i, _ -> i != index }) },
                    )
                }
                OrbitButton(
                    "ADD RULE",
                    onClick = {
                        val field = fields.first()
                        val newRule = SmartRuleInfo(field.key, field.ops.firstOrNull()?.key.orEmpty(), "")
                        draft = current.copy(rules = current.rules + newRule)
                    },
                    icon = OrbitIcons.Plus,
                    enabled = fields.isNotEmpty(),
                )

                SectionTitle("SORT", Modifier.padding(top = Space.Compact))
                OrbitDropdown(
                    options = sortOptions,
                    selectedKey = current.sort,
                    onSelect = { draft = current.copy(sort = it) },
                    modifier = Modifier.fillMaxWidth(),
                )

                SectionTitle("LIMIT", Modifier.padding(top = Space.Compact))
                OrbitTextField(
                    value = if (current.limit == 0u) "" else current.limit.toString(),
                    onValueChange = { text ->
                        val digits = text.filter { it.isDigit() }.take(6)
                        draft = current.copy(limit = digits.toUIntOrNull() ?: 0u)
                    },
                    modifier = Modifier.fillMaxWidth(),
                    placeholder = "0 = NO LIMIT",
                    keyboardType = KeyboardType.Number,
                )

                PreviewBlock(preview, previewFailed)

                Column(
                    Modifier.fillMaxWidth().padding(top = Space.S, bottom = Space.Xl),
                    verticalArrangement = Arrangement.spacedBy(Space.S),
                ) {
                    Row(
                        Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(Dimens.RowPadV),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        OrbitButton("CANCEL", onClick = { env.nav.pop() }, modifier = Modifier.weight(1f))
                        OrbitButton(
                            "SAVE",
                            onClick = {
                                saving = true
                                env.collections.saveSmart(current.copy(name = current.name.trim())) { env.nav.pop() }
                            },
                            modifier = Modifier.weight(1f),
                            primary = true,
                            enabled = current.name.isNotBlank() && !saving,
                        )
                    }
                    if (id != 0L) {
                        OrbitButton(
                            "DELETE SMART PLAYLIST",
                            onClick = { confirmDelete = true },
                            modifier = Modifier.fillMaxWidth(),
                            danger = true,
                        )
                    }
                }
            }
        }
    }

    if (confirmDelete) {
        ConfirmDialog(
            message = "DELETE THIS SMART PLAYLIST? THIS CANNOT BE UNDONE.",
            confirmLabel = "DELETE",
            onConfirm = {
                confirmDelete = false
                // The playlist screen underneath would show "not found": go back to the tab root.
                env.collections.deleteSmart(id) { env.nav.popToRoot() }
            },
            onDismiss = { confirmDelete = false },
        )
    }
}

/** One rule: field and operator dropdowns, a value field when the operator needs one, and a remove button. */
@Composable
private fun RuleCard(
    rule: SmartRuleInfo,
    fields: List<SmartFieldInfo>,
    onChange: (SmartRuleInfo) -> Unit,
    onRemove: () -> Unit,
) {
    val fieldOptions = remember(fields) { fields.map { it.key to it.label } }
    val fieldsByKey = remember(fields) { fields.associateBy { it.key } }
    val field = fieldsByKey[rule.field]
    val opOptions = remember(field) { field?.ops.orEmpty().map { it.key to it.label } }
    val opsByKey = remember(field) { field?.ops.orEmpty().associateBy { it.key } }
    val op = opsByKey[rule.op]
    Column(
        Modifier.fillMaxWidth().background(Orbit.Panel).border(BorderStroke(Dimens.Stroke, Orbit.Line)).padding(Space.M),
        verticalArrangement = Arrangement.spacedBy(Space.S),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(Space.S), verticalAlignment = Alignment.CenterVertically) {
            OrbitDropdown(
                options = fieldOptions,
                selectedKey = rule.field,
                onSelect = { key ->
                    if (key != rule.field) {
                        val next = fieldsByKey[key]
                        onChange(SmartRuleInfo(key, next?.ops?.firstOrNull()?.key.orEmpty(), ""))
                    }
                },
                modifier = Modifier.weight(1f),
            )
            Box(Modifier.size(Dimens.Touch).clickable(onClick = onRemove), contentAlignment = Alignment.Center) {
                Icon(OrbitIcons.X, "Remove rule", tint = Orbit.TextDim, modifier = Modifier.size(Dimens.IconMd))
            }
        }
        OrbitDropdown(
            options = opOptions,
            selectedKey = rule.op,
            onSelect = { onChange(rule.copy(op = it)) },
            modifier = Modifier.fillMaxWidth(),
        )
        if (op == null || op.needsValue) {
            OrbitTextField(
                value = rule.value,
                onValueChange = { onChange(rule.copy(value = it)) },
                modifier = Modifier.fillMaxWidth(),
                placeholder = field?.hint.orEmpty(),
                keyboardType = if (rule.field in NumericFields) KeyboardType.Number else KeyboardType.Text,
            )
        }
    }
}

/** `N TRACKS MATCH` in gold plus the first few matches inside a framed panel. */
@Composable
private fun PreviewBlock(preview: SmartPreview?, failed: Boolean) {
    Column(Modifier.fillMaxWidth().padding(top = Space.Compact), verticalArrangement = Arrangement.spacedBy(Space.S)) {
        SectionTitle("PREVIEW")
        Column(
            Modifier.fillMaxWidth().background(Orbit.Panel).border(BorderStroke(Dimens.Stroke, Orbit.LineSoft)).padding(Space.M),
            verticalArrangement = Arrangement.spacedBy(Space.S),
        ) {
            when {
                preview == null -> Text(
                    if (failed) "CHECK THE RULES · NO PREVIEW" else "LOADING…",
                    color = Orbit.TextDim, style = OrbitType.Status,
                )
                else -> {
                    val count = preview.count.toInt()
                    Text(
                        if (failed) "CHECK THE RULES · SHOWING LAST RESULT" else "${pluralCount(count, "TRACK", "TRACKS")} MATCH",
                        color = if (failed) Orbit.TextDim else Orbit.Gold,
                        style = OrbitType.CountBold,
                    )
                    if (preview.tracks.isNotEmpty()) Divider()
                    preview.tracks.take(PreviewRows).forEach { track ->
                        Row(
                            Modifier.fillMaxWidth().padding(vertical = Space.Hair),
                            horizontalArrangement = Arrangement.spacedBy(Space.S),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f)) {
                                Text(
                                    track.title,
                                    color = Orbit.Text, style = OrbitType.PreviewTitle,
                                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                                )
                                Text(
                                    track.artist,
                                    color = Orbit.TextDim, style = OrbitType.PreviewMeta,
                                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                                )
                            }
                            Text(formatDuration(track.durationSecs), color = Orbit.TextDim, style = OrbitType.Status)
                        }
                    }
                    if (count > PreviewRows) {
                        Text("+ ${count - PreviewRows} MORE", color = Orbit.TextMute, style = OrbitType.Status)
                    }
                }
            }
        }
    }
}
