package dev.okarin.rustygrade.ui.screens

import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import kotlinx.coroutines.delay
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.BackupUi
import dev.okarin.rustygrade.FetchUi
import dev.okarin.rustygrade.ScanUi
import dev.okarin.rustygrade.ui.AudioSettings
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.OrbitButton
import dev.okarin.rustygrade.ui.components.OrbitChip
import dev.okarin.rustygrade.ui.components.OrbitSlider
import dev.okarin.rustygrade.ui.components.OrbitTextField
import dev.okarin.rustygrade.ui.components.ScreenHeader
import dev.okarin.rustygrade.ui.components.SpectrumBar
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import uniffi.rustygrade_ffi.coreVersion
import kotlin.math.roundToInt

/** Settings tab: library sources and scan, appearance, audio, system and about panels. */
@Composable
fun SettingsScreen() {
    val env = LocalEnv.current
    val context = LocalContext.current
    val settings by env.library.settings.collectAsStateWithLifecycle()
    val library by env.library.library.collectAsStateWithLifecycle()
    val scan by env.library.scan.collectAsStateWithLifecycle()
    val appVersion = remember { appVersionName(context) }

    val status = "VER $appVersion"

    Column(Modifier.fillMaxSize()) {
        ScreenHeader("SYSTEM_CONFIG", status = status)
        Column(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = Space.Gutter, vertical = Space.S),
            verticalArrangement = Arrangement.spacedBy(Space.Xxl),
        ) {
            SettingsPanel("LIBRARY") {
                val s = settings
                if (s != null) {
                    ToggleRow(
                        "MEDIASTORE",
                        s.useMediaStore,
                        { env.library.setUseMediaStore(it) },
                        caption = "Index device audio.",
                    )
                    ToggleRow(
                        "SCAN ON STARTUP",
                        s.scanOnStartup,
                        { env.library.setScanOnStartup(it) },
                        caption = "Scan library on launch.",
                    )
                    ToggleRow(
                        "AUTO-SCAN REPLAYGAIN",
                        s.autoReplayGain,
                        { env.library.setAutoReplayGain(it) },
                        caption = "Auto-scan EBU R128 on new tracks.",
                    )
                    if (s.safTrees.isEmpty()) {
                        Caption("NO FOLDERS ADDED")
                    } else {
                        Column(verticalArrangement = Arrangement.spacedBy(Space.Xs)) {
                            s.safTrees.forEach { uri ->
                                FolderRow(folderName(uri), onRemove = { env.library.removeTreeAndRescan(uri) })
                            }
                        }
                    }
                    Divider()
                }

                val running = scan is ScanUi.Running
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.S)) {
                    OrbitButton("ADD FOLDER", onClick = { env.addFolder() }, modifier = Modifier.weight(1f), icon = OrbitIcons.Folderplus)
                    if (running) {
                        OrbitButton("CANCEL", onClick = { env.library.cancelScan() }, modifier = Modifier.weight(1f), danger = true)
                    } else {
                        OrbitButton("RESCAN", onClick = { env.scan() }, modifier = Modifier.weight(1f), primary = true)
                    }
                }
                OrbitButton(
                    "RESYNC COVERS",
                    onClick = { env.library.resyncCovers() },
                    modifier = Modifier.fillMaxWidth(),
                    icon = OrbitIcons.Refresh,
                    enabled = !running,
                )
                ScanStatus(scan)
            }

            SettingsPanel("APPEARANCE") {
                val s = settings
                if (s != null) {
                    ToggleRow(
                        "DYNAMIC COVER ACCENT",
                        s.accentFromCover,
                        { env.library.setAccentFromCover(it) },
                        caption = "Tint UI from playing cover.",
                    )
                }
            }

            SettingsPanel("AUDIO") {
                AudioSettings()
                if (settings != null) Divider()
                CrossfadeRow()
            }

            FetchAnalyzePanel()

            BackupSyncPanel()

            SettingsPanel("SYSTEM") {
                Row(
                    Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Space.M),
                ) {
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                        SettingLabel("BATTERY OPTIMISATION")
                        Caption("Exempt app if background playback stops.")
                    }
                    OrbitButton("OPEN", onClick = { openBatterySettings(context) })
                }
            }

            SettingsPanel("ABOUT") {
                val coreVer = remember { coreVersion() }
                Column(verticalArrangement = Arrangement.spacedBy(Space.S)) {
                    Text(
                        buildAnnotatedString {
                            withStyle(SpanStyle(color = Orbit.Text, fontWeight = FontWeight.Bold)) { append("RUSTYGRADE") }
                            withStyle(SpanStyle(color = Orbit.TextMute)) { append(" // ") }
                            withStyle(SpanStyle(color = Orbit.Teal)) { append("VER $coreVer") }
                        },
                        style = OrbitType.Hud,
                    )
                    SpectrumBar()
                }
                Caption("APP VERSION $appVersion")
            }
            Spacer(Modifier.height(Space.S))
        }
    }
}

/** Batch online lyrics/cover lookup and in-process EBU R128 ReplayGain scanner. */
@Composable
private fun FetchAnalyzePanel() {
    val env = LocalEnv.current
    val fetchUi by env.library.fetchUi.collectAsStateWithLifecycle()
    val scan by env.library.scan.collectAsStateWithLifecycle()
    val library by env.library.library.collectAsStateWithLifecycle()
    val settings by env.library.settings.collectAsStateWithLifecycle()
    val running = fetchUi is FetchUi.Running
    val canStart = !running && scan !is ScanUi.Running && (library?.trackCount ?: 0u) > 0u

    SettingsPanel("FETCH_&_ANALYZE") {
        Caption("Fetch missing lyrics/covers or scan EBU R128 ReplayGain.")
        settings?.let { s ->
            SettingLabel("ARTWORK SOURCES")
            Row(horizontalArrangement = Arrangement.spacedBy(Space.Compact)) {
                OrbitChip("ITUNES", s.artworkItunes, {
                    env.library.setArtworkSources(!s.artworkItunes, s.artworkDeezer, s.artworkMusicbrainz)
                })
                OrbitChip("DEEZER", s.artworkDeezer, {
                    env.library.setArtworkSources(s.artworkItunes, !s.artworkDeezer, s.artworkMusicbrainz)
                })
                OrbitChip("MUSICBRAINZ", s.artworkMusicbrainz, {
                    env.library.setArtworkSources(s.artworkItunes, s.artworkDeezer, !s.artworkMusicbrainz)
                })
            }
            Divider()
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.S)) {
            OrbitButton(
                "FETCH LYRICS",
                onClick = { env.library.fetchMissingLyrics() },
                modifier = Modifier.weight(1f),
                icon = OrbitIcons.Download,
                enabled = canStart,
            )
            OrbitButton(
                "FETCH COVERS",
                onClick = { env.library.fetchMissingCovers() },
                modifier = Modifier.weight(1f),
                icon = OrbitIcons.Image,
                enabled = canStart,
            )
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.S)) {
            OrbitButton(
                "SCAN REPLAYGAIN",
                onClick = { env.library.scanReplayGain(forceAll = false) },
                modifier = Modifier.weight(1f),
                icon = OrbitIcons.Sliders,
                primary = true,
                enabled = canStart,
            )
            if (running) {
                OrbitButton(
                    "CANCEL",
                    onClick = { env.library.cancelFetch() },
                    modifier = Modifier.weight(1f),
                    danger = true,
                )
            } else {
                OrbitButton(
                    "RESCAN ALL RG",
                    onClick = { env.library.scanReplayGain(forceAll = true) },
                    modifier = Modifier.weight(1f),
                    icon = OrbitIcons.Refresh,
                    enabled = canStart,
                )
            }
        }
        when (val f = fetchUi) {
            FetchUi.Idle -> Unit
            is FetchUi.Running -> {
                val progress = if (f.total > 0) "${f.done}/${f.total}" else "…"
                val extra = if (f.kind != "REPLAYGAIN" && f.total > 0) " // ${f.found} FOUND" else ""
                val detail = if (f.detail.isNotBlank()) " // ${f.detail}" else ""
                Caption("${f.kind} // $progress$extra$detail", color = Orbit.Gold)
            }
            is FetchUi.Done -> Caption("DONE // ${f.message}", color = Orbit.Teal)
            is FetchUi.Failed -> Caption("FAILED // ${f.message}", color = Orbit.Danger)
        }
    }
}

/** Backup & sync panel: ZIP export/import compatible with `~/.rustygrade`. */
@Composable
private fun BackupSyncPanel() {
    val env = LocalEnv.current
    val backupUi by env.library.backupUi.collectAsStateWithLifecycle()
    val pendingReplace by env.library.pendingReplace.collectAsStateWithLifecycle()
    val library by env.library.library.collectAsStateWithLifecycle()
    var replaceMode by rememberSaveable { mutableStateOf(false) }
    var sourceRoot by rememberSaveable { mutableStateOf("") }
    var advancedExpanded by rememberSaveable { mutableStateOf(false) }
    val showSourceRoot = advancedExpanded || sourceRoot.isNotEmpty()
    val busy = backupUi is BackupUi.Busy

    val exportLauncher = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/zip")) { uri: Uri? ->
        if (uri != null && !busy) env.library.exportBackup(uri)
    }

    val importLauncher = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri == null || busy) return@rememberLauncherForActivityResult
        if (replaceMode) {
            env.library.inspectBackupForReplace(uri)
        } else {
            env.library.importBackup(uri, replace = false, sourceRoot = sourceRoot)
        }
    }

    SettingsPanel("BACKUP_&_SYNC") {
        Caption("Export or import playlists, stats, and settings (.zip).")
        if (library?.trackCount == 0u) {
            Caption("Imported paths remap automatically after scanning.", color = Orbit.Gold)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(Space.S)) {
            OrbitChip("MERGE", !replaceMode, { if (!busy) replaceMode = false })
            OrbitChip("REPLACE", replaceMode, { if (!busy) replaceMode = true })
        }
        Caption(
            if (replaceMode) "Overwrites existing data." else "Adds new items, keeps higher play counts.",
        )
        Divider()
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = Dimens.Touch)
                .pressable(pressedScale = 0.985f) { advancedExpanded = !showSourceRoot },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Space.S),
        ) {
            Text(
                "ADVANCED: DESKTOP MUSIC ROOT",
                modifier = Modifier.weight(1f),
                color = Orbit.TextDim,
                style = OrbitType.Kicker,
            )
            Text(
                if (showSourceRoot) "−" else "+",
                color = Orbit.Gold,
                style = OrbitType.ScreenTitle,
            )
        }
        if (showSourceRoot) {
            OrbitTextField(
                value = sourceRoot,
                onValueChange = { sourceRoot = it },
                modifier = Modifier.fillMaxWidth(),
                placeholder = "AUTO-DETECT (E.G. /HOME/USER/MUSIC)",
            )
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.S)) {
            OrbitButton(
                "EXPORT ZIP",
                onClick = {
                    val stamp = SimpleDateFormat("yyyyMMdd", Locale.US).format(Date())
                    exportLauncher.launch("rustygrade-backup-$stamp.zip")
                },
                modifier = Modifier.weight(1f),
                icon = OrbitIcons.Download,
                enabled = !busy,
            )
            OrbitButton(
                "IMPORT ZIP",
                onClick = {
                    importLauncher.launch(
                        arrayOf("application/zip", "application/x-zip-compressed", "application/octet-stream"),
                    )
                },
                modifier = Modifier.weight(1f),
                icon = OrbitIcons.Upload,
                primary = true,
                enabled = !busy,
            )
        }
        when (val b = backupUi) {
            BackupUi.Idle -> Unit
            BackupUi.Busy -> Caption("PROCESSING BACKUP...", color = Orbit.Gold)
            is BackupUi.Done -> Caption(b.message, color = Orbit.Teal)
            is BackupUi.Failed -> Caption(b.message, color = Orbit.Danger)
        }
    }

    pendingReplace?.let { pending ->
        val p = pending.preview
        val title = if (p.isSinglePlaylistExport) "SINGLE PLAYLIST ARCHIVE" else "WARNING"
        val message = if (p.isSinglePlaylistExport) {
            "THIS ARCHIVE ONLY EXPORTS ONE PLAYLIST (\"${p.singlePlaylistName ?: "PLAYLIST"}\", 0 FAVORITES, 0 SMART PLAYLISTS).\n\n" +
                "REPLACING WILL WIPE ALL EXISTING PLAYLISTS, FAVORITES AND CUSTOM SMART PLAYLISTS."
        } else {
            val sections = buildList {
                if (p.hasCollections) add("${p.playlistCount} PLAYLISTS · ${p.likedCount} LIKED · ${p.smartPlaylistCount} SMART")
                if (p.hasStats) add("PLAY STATS")
                if (p.hasSettings) add("SETTINGS")
                if (p.coverCount > 0u) add("${p.coverCount} COVERS")
            }.joinToString(" · ")
            "REPLACE CURRENT DATA WITH ARCHIVE ($sections)? THIS CANNOT BE UNDONE."
        }
        AlertDialog(
            onDismissRequest = { env.library.dismissPendingReplace() },
            shape = RectangleShape,
            containerColor = Orbit.Raised,
            title = {
                Text(title, color = Orbit.Danger, style = OrbitType.ScreenTitle)
            },
            text = { Text(message, color = Orbit.Text, style = OrbitType.Body) },
            confirmButton = {
                Row(horizontalArrangement = Arrangement.spacedBy(Space.S)) {
                    if (p.isSinglePlaylistExport) {
                        TextButton(onClick = { env.library.confirmPendingImport(replace = false, sourceRoot = sourceRoot) }) {
                            Text("MERGE INSTEAD", color = Orbit.Teal, style = OrbitType.Button)
                        }
                    }
                    TextButton(onClick = { env.library.confirmPendingImport(replace = true, sourceRoot = sourceRoot) }) {
                        Text(if (p.isSinglePlaylistExport) "REPLACE ALL" else "REPLACE", color = Orbit.Danger, style = OrbitType.Button)
                    }
                }
            },
            dismissButton = {
                TextButton(onClick = { env.library.dismissPendingReplace() }) {
                    Text("CANCEL", color = Orbit.TextDim, style = OrbitType.Button)
                }
            },
        )
    }
}

/** Crossfade length 0..12 s (0 = off); applied when the slider is released. */
@Composable
private fun CrossfadeRow() {
    val env = LocalEnv.current
    val settings by env.library.settings.collectAsStateWithLifecycle()
    val s = settings ?: return
    var seconds by remember(s.crossfadeSeconds) { mutableFloatStateOf(s.crossfadeSeconds.toFloat()) }
    val value = seconds.roundToInt().coerceIn(0, 12)

    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            SettingLabel("CROSSFADE", Modifier.weight(1f))
            Text(
                if (value == 0) "OFF" else "$value S",
                color = if (value == 0) Orbit.TextDim else Orbit.Teal,
                style = OrbitType.Numeric,
            )
        }
        OrbitSlider(
            value = seconds,
            valueRange = 0f..12f,
            steps = 11,
            accent = Orbit.Teal,
            contentDescription = "Crossfade duration",
            onValueChange = { seconds = it },
            onValueChangeFinished = {
                val chosen = seconds.roundToInt().coerceIn(0, 12)
                env.library.setCrossfadeSeconds(chosen) // saves, refreshes the settings flow and the service
            },
        )
        Caption("Gapless albums stay untouched; off in Direct USB.")
    }
}

@Composable
private fun FolderRow(name: String, onRemove: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = Dimens.Touch)
            .background(Orbit.PanelDeep)
            .border(BorderStroke(Dimens.Stroke, Orbit.LineSoft))
            .padding(start = Space.M, end = Space.Xs),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            name,
            modifier = Modifier.weight(1f),
            color = Orbit.Text,
            style = OrbitType.Numeric,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Box(Modifier.size(Dimens.Touch).pressable(pressedScale = 0.9f, onClick = onRemove), contentAlignment = Alignment.Center) {
            Icon(OrbitIcons.X, "Remove folder", tint = Orbit.TextDim, modifier = Modifier.size(Dimens.IconS))
        }
    }
}

@Composable
private fun ScanStatus(scan: ScanUi) {
    // A finished (or failed) scan / cover resync reports once, then the line clears itself.
    var hidden by remember(scan) { mutableStateOf(false) }
    LaunchedEffect(scan) {
        if (scan is ScanUi.Done || scan is ScanUi.Failed) {
            delay(SCAN_STATUS_AUTO_HIDE_MS)
            hidden = true
        }
    }
    if (hidden) return
    when (scan) {
        ScanUi.Idle -> Unit
        is ScanUi.Running -> Caption(
            "SCANNING // ${scan.phase.uppercase()} // ${scan.scanned}/${scan.total}",
            color = Orbit.Gold,
        )
        is ScanUi.Done -> {
            val s = scan.summary
            val cov = if (s.covers > 0u) " // ${s.covers} COVERS" else ""
            Caption(
                "DONE // +${s.addedTracks} ADDED // ${s.updatedTracks} UPDATED // -${s.removedTracks} REMOVED$cov",
                color = Orbit.Teal,
            )
        }
        is ScanUi.Failed -> Caption("FAILED // ${scan.message}", color = Orbit.Danger)
    }
}

/** How long the settings scan / resync result line stays before clearing itself. */
private const val SCAN_STATUS_AUTO_HIDE_MS = 5_000L

/** `content://.../tree/primary%3AMusic` -> `primary:Music`. */
private fun folderName(treeUri: String): String {
    val segment = runCatching { Uri.parse(treeUri).lastPathSegment }.getOrNull() ?: return treeUri
    return Uri.decode(segment)
}

private fun openBatterySettings(context: android.content.Context) {
    try {
        context.startActivity(
            Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    } catch (_: ActivityNotFoundException) {
        // No such settings screen on this device.
    }
}

private fun appVersionName(context: android.content.Context): String = try {
    val pm = context.packageManager
    val info = if (Build.VERSION.SDK_INT >= 33) {
        pm.getPackageInfo(context.packageName, PackageManager.PackageInfoFlags.of(0))
    } else {
        @Suppress("DEPRECATION")
        pm.getPackageInfo(context.packageName, 0)
    }
    info.versionName ?: "?"
} catch (_: Exception) {
    "?"
}
