package dev.okarin.rustygrade.ui.screens

import android.app.Activity
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.activity.compose.LocalActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import coil3.compose.AsyncImage
import dev.okarin.rustygrade.TagEditorViewModel
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.Cover
import dev.okarin.rustygrade.ui.components.CoverPickerSheet
import dev.okarin.rustygrade.ui.components.EmptyMessage
import dev.okarin.rustygrade.ui.components.LocalNowPlayingId
import dev.okarin.rustygrade.ui.components.LyricPickerSheet
import dev.okarin.rustygrade.ui.components.OrbitButton
import dev.okarin.rustygrade.ui.components.OrbitMultilineTextField
import dev.okarin.rustygrade.ui.components.ScreenHeader
import dev.okarin.rustygrade.ui.components.SectionTitle
import dev.okarin.rustygrade.ui.components.pluralCount
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.nav.Route
import dev.okarin.rustygrade.ui.nav.TagTarget
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.Space

/**
 * Edit the tags (and cover) of one track or of every track of an album. Only fields the user
 * changed are written; session state is held in [TagEditorViewModel] so rotating during the
 * system write permission dialog or write does not lose edits or pending state.
 */
@Composable
fun TagEditorScreen(target: TagTarget, vm: TagEditorViewModel = viewModel()) {
    val env = LocalEnv.current
    val activity = LocalActivity.current

    BackHandler(enabled = vm.saving) {}
    LaunchedEffect(target) { vm.ensureLoaded(target) }

    LaunchedEffect(vm.reloadTick) {
        if (vm.reloadTick > 0) env.library.reload()
    }
    LaunchedEffect(vm.completedEvent) {
        val done = vm.completedEvent ?: return@LaunchedEffect
        vm.consumeCompleted()
        vm.clearSession()
        env.nav.pop()
        val renamed = done.renamedAlbum
        val old = (done.target as? TagTarget.Album)?.key
        // The album screen below was opened for the old name: follow the rename.
        if (renamed != null && old != null && renamed != old && env.nav.current == Route.Album(old)) {
            env.nav.pop()
            env.nav.push(Route.Album(renamed))
        }
    }

    DisposableEffect(target) {
        onDispose {
            if (activity?.isChangingConfigurations != true && !vm.saving) {
                vm.clearSession()
            }
        }
    }

    when (val state = vm.loadState) {
        TagEditorViewModel.LoadState.Loading -> Column(Modifier.fillMaxSize()) {
            ScreenHeader("EDIT TAGS", onBack = { if (!vm.saving) { vm.clearSession(); env.nav.pop() } })
        }
        TagEditorViewModel.LoadState.Missing -> Column(Modifier.fillMaxSize()) {
            ScreenHeader("EDIT TAGS", onBack = { if (!vm.saving) { vm.clearSession(); env.nav.pop() } })
            EmptyMessage(if (target is TagTarget.Album) "ALBUM NOT FOUND" else "TRACK NOT FOUND")
        }
        is TagEditorViewModel.LoadState.Ready -> TagEditorBody(target, state.data, state.form, vm)
    }
}

@Composable
private fun TagEditorBody(
    target: TagTarget,
    data: TagLoad,
    form: TagForm,
    vm: TagEditorViewModel,
) {
    val env = LocalEnv.current
    val tracks = data.tracks
    val albumMode = target is TagTarget.Album

    val playingId = LocalNowPlayingId.current
    val touchesPlaying = playingId != null && tracks.any { it.id == playingId }

    val accessLauncher = rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) { result ->
        val edit = vm.pendingEdit
        if (result.resultCode == Activity.RESULT_OK) {
            if (edit != null) vm.launchWrite(tracks, edit, env.playback) else vm.onSessionResetAfterGrant()
        } else {
            vm.onAccessDenied()
        }
    }

    LaunchedEffect(vm.pendingWriteRequest) {
        val sender = vm.pendingWriteRequest ?: return@LaunchedEffect
        vm.consumeWriteRequest()
        try {
            accessLauncher.launch(IntentSenderRequest.Builder(sender).build())
        } catch (e: Exception) {
            // The system cannot show the write-access dialog: do not leave the editor stuck in "saving".
            vm.onAccessDenied()
        }
    }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri: Uri? ->
        if (uri != null) vm.pickCover(uri)
    }
    var showCoverPicker by remember { mutableStateOf(false) }
    var showLyricPicker by remember { mutableStateOf(false) }

    Column(Modifier.fillMaxSize()) {
        ScreenHeader("EDIT TAGS", onBack = { if (!vm.saving) { vm.clearSession(); env.nav.pop() } })
        Column(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .imePadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = Space.L, vertical = Space.S),
            verticalArrangement = Arrangement.spacedBy(Dimens.RowGap),
        ) {
            Caption(
                if (albumMode) "${pluralCount(tracks.size, "TRACK", "TRACKS")} · Changed fields apply to all tracks." else tracks[0].title,
                color = Orbit.TextDim,
            )

            // Cover.
            Row(horizontalArrangement = Arrangement.spacedBy(Space.M), verticalAlignment = Alignment.Top) {
                val preview = vm.coverFile
                when {
                    vm.removeCover -> Cover(null, size = 112.dp)
                    preview != null -> AsyncImage(
                        preview, null,
                        Modifier.size(112.dp).background(Orbit.Card).border(BorderStroke(Dimens.Stroke, Orbit.Line)),
                        contentScale = ContentScale.Crop,
                    )
                    else -> Cover(data.coverPath, size = 112.dp)
                }
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.S)) {
                    OrbitButton(
                        "CHANGE COVER",
                        onClick = { picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
                        modifier = Modifier.fillMaxWidth(),
                        icon = OrbitIcons.Image,
                        enabled = !vm.saving && !vm.pickingCover,
                    )
                    OrbitButton(
                        "FETCH COVER",
                        onClick = { showCoverPicker = true },
                        modifier = Modifier.fillMaxWidth(),
                        icon = OrbitIcons.Download,
                        enabled = !vm.saving && !vm.pickingCover,
                    )
                    OrbitButton(
                        "REMOVE COVER",
                        onClick = { vm.markRemoveCover() },
                        modifier = Modifier.fillMaxWidth(),
                        danger = true,
                        enabled = !vm.saving && !vm.pickingCover && !vm.removeCover && (data.coverPath != null || vm.coverFile != null),
                    )
                    when {
                        vm.removeCover -> Caption("Cover will be removed.", color = Orbit.Gold)
                        vm.coverFile != null -> Caption("New cover selected.", color = Orbit.Gold)
                    }
                    Caption("Cover changes apply to every track being edited.")
                }
            }

            if (!albumMode) TagFieldRow("TITLE", form.title)
            TagFieldRow("ARTIST", form.artist)
            TagFieldRow("ALBUM", form.albumName)
            TagFieldRow("ALBUM ARTIST", form.albumArtist)
            Row(horizontalArrangement = Arrangement.spacedBy(Space.S)) {
                TagFieldRow("YEAR", form.year, Modifier.weight(1f), numeric = true)
                if (!albumMode) TagFieldRow("TRACK #", form.track, Modifier.weight(1f), numeric = true)
                TagFieldRow("DISC #", form.disc, Modifier.weight(1f), numeric = true)
            }

            if (!albumMode) {
                Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.S)) {
                    Row(
                        Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.SpaceBetween,
                    ) {
                        SectionTitle("LYRICS")
                        OrbitButton(
                            "FETCH LYRICS",
                            onClick = { showLyricPicker = true },
                            icon = OrbitIcons.Download,
                            enabled = !vm.saving,
                        )
                    }
                    OrbitMultilineTextField(
                        value = form.lyrics.text,
                        onValueChange = { form.lyrics.text = it },
                        modifier = Modifier.fillMaxWidth(),
                        placeholder = form.lyrics.placeholder,
                    )
                }
            }

            if (touchesPlaying) Caption("Playback pauses while tags are written.")

            // Status.
            if (vm.saving) Caption("SAVING ${vm.progress}/${tracks.size}", color = Orbit.Gold)
            vm.error?.let { Caption(it, color = Orbit.Danger) }
            vm.updated?.let { n ->
                if (n > 0) Caption("${pluralCount(n, "TRACK", "TRACKS")} UPDATED", color = Orbit.Teal)
            }
            vm.failures.forEach { (track, message) ->
                Caption("${track.title}: $message", color = Orbit.Danger)
            }

            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.S)) {
                OrbitButton(
                    "CANCEL",
                    onClick = {
                        vm.clearSession()
                        env.nav.pop()
                    },
                    modifier = Modifier.weight(1f),
                    enabled = !vm.saving,
                )
                OrbitButton(
                    "SAVE",
                    onClick = { vm.beginSave(env.playback) },
                    modifier = Modifier.weight(1f),
                    primary = true,
                    enabled = !vm.saving && !vm.pickingCover,
                )
            }
            Spacer(Modifier.height(Space.S))
        }
    }

    if (showCoverPicker) {
        CoverPickerSheet(
            initialAlbum = form.albumName.text.ifBlank { data.album?.name ?: tracks[0].album },
            initialArtist = form.albumArtist.text.ifBlank { form.artist.text.ifBlank { tracks[0].artist } },
            trackTitle = form.title.text.ifBlank { tracks[0].title },
            year = form.year.text.toIntOrNull() ?: data.album?.year ?: 0,
            onDismiss = { showCoverPicker = false },
            onPickedFile = { file -> vm.setFetchedCover(file) },
        )
    }

    if (showLyricPicker && !albumMode) {
        val base = tracks[0]
        val queryTrack = base.copy(
            title = form.title.text.ifBlank { base.title },
            artist = form.artist.text.ifBlank { base.artist },
            album = form.albumName.text.ifBlank { base.album },
        )
        LyricPickerSheet(
            track = queryTrack,
            onDismiss = { showLyricPicker = false },
            onPickedText = { text -> form.lyrics.text = text },
        )
    }
}
