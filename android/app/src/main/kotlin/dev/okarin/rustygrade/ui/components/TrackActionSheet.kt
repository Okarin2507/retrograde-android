package dev.okarin.rustygrade.ui.components

import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.BottomSheetDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.nav.Route
import dev.okarin.rustygrade.ui.nav.TagTarget
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import uniffi.rustygrade_ffi.TrackInfo

/** One row of an action sheet. */
data class SheetAction(
    val label: String,
    val icon: ImageVector,
    val enabled: Boolean = true,
    val danger: Boolean = false,
    val onClick: () -> Unit,
)

/**
 * The `⋮` menu of a track: PLAY NEXT, ADD TO QUEUE, FAVORITE, ADD TO PLAYLIST, EDIT TAGS, plus the
 * caller's [extraActions] (e.g. REMOVE FROM PLAYLIST).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TrackActionSheet(track: TrackInfo, onDismiss: () -> Unit, extraActions: List<SheetAction> = emptyList()) {
    val env = LocalEnv.current
    val context = LocalContext.current
    var picking by remember { mutableStateOf(false) }
    val isLiked = track.id in LocalLikedIds.current

    fun say(text: String) = Toast.makeText(context, text, Toast.LENGTH_SHORT).show()

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        shape = RectangleShape,
        containerColor = Orbit.PanelDeep,
        contentColor = Orbit.Text,
        dragHandle = { BottomSheetDefaults.DragHandle(color = Orbit.Line) },
    ) {
        Column(Modifier.navigationBarsPadding().padding(bottom = Space.M)) {
            Row(
                Modifier.padding(horizontal = Space.Gutter, vertical = Dimens.RowPadV),
                horizontalArrangement = Arrangement.spacedBy(Space.M),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Cover(track.thumbPath, size = Dimens.SheetCover)
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                    Text(
                        track.title,
                        color = Orbit.Text,
                        style = OrbitType.RowTitle,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        "${track.artist} · ${track.album}",
                        color = Orbit.TextDim,
                        style = OrbitType.RowMeta,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            Divider()
            if (picking) {
                PlaylistPicker(track, onDone = { name ->
                    say("ADDED TO $name")
                    onDismiss()
                })
            } else {
                val actions = listOf(
                    SheetAction("PLAY NEXT", OrbitIcons.Playnext) {
                        env.playback.playNext(listOf(track.id)); say("PLAYS NEXT"); onDismiss()
                    },
                    SheetAction("ADD TO QUEUE", OrbitIcons.Listmusic) {
                        env.playback.enqueue(listOf(track.id)); say("ADDED TO QUEUE"); onDismiss()
                    },
                    SheetAction(
                        if (isLiked) "REMOVE FROM FAVORITES" else "FAVORITE",
                        if (isLiked) OrbitIcons.Heartfilled else OrbitIcons.Heart,
                    ) {
                        env.collections.toggleLiked(track.id); onDismiss()
                    },
                    SheetAction("ADD TO PLAYLIST", OrbitIcons.Plus) { picking = true },
                    if (track.album.isNotBlank()) {
                        SheetAction("GO TO ALBUM", OrbitIcons.Disc) {
                            env.nav.playerOpen = false
                            env.nav.push(Route.Album(track.album))
                            onDismiss()
                        }
                    } else null,
                    if (track.artist.isNotBlank()) {
                        SheetAction("GO TO ARTIST", OrbitIcons.User) {
                            env.nav.playerOpen = false
                            env.nav.push(Route.Artist(track.artist))
                            onDismiss()
                        }
                    } else null,
                    SheetAction("EDIT TAGS", OrbitIcons.Pencil) {
                        env.nav.playerOpen = false // the editor is a screen under the full-screen player
                        env.nav.push(Route.TagEditor(TagTarget.Track(track.id)))
                        onDismiss()
                    },
                ).filterNotNull() + extraActions
                // Scrolls on its own: in landscape the sheet is shorter than its actions.
                Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
                    actions.forEach { ActionRow(it) }
                }
            }
        }
    }
}

@Composable
fun ActionRow(action: SheetAction) {
    val tint = when {
        !action.enabled -> Orbit.TextMute
        action.danger -> Orbit.Danger
        else -> Orbit.Text
    }
    Row(
        Modifier.fillMaxWidth()
            .then(if (action.enabled) Modifier.pressable(pressedScale = 0.985f, onClick = action.onClick) else Modifier)
            .padding(horizontal = Space.Gutter, vertical = Dimens.RowGap),
        horizontalArrangement = Arrangement.spacedBy(Dimens.RowGap),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(action.icon, null, tint = tint, modifier = Modifier.size(Dimens.IconMd))
        Text(
            action.label,
            color = tint,
            style = OrbitType.Button,
        )
    }
}

/** Pick an existing playlist, or create one on the spot, and add [track] (or [trackIds]) to it. */
@Composable
fun PlaylistPicker(track: TrackInfo, onDone: (String) -> Unit) = PlaylistPicker(listOf(track.id), onDone)

@Composable
fun PlaylistPicker(trackIds: List<String>, onDone: (playlistName: String) -> Unit) {
    val env = LocalEnv.current
    val snapshot by env.collections.collections.collectAsStateWithLifecycle()
    var name by remember { mutableStateOf("") }

    Column {
        OrbitTextField(
            value = name,
            onValueChange = { name = it },
            modifier = Modifier.fillMaxWidth().padding(horizontal = Space.Gutter, vertical = Dimens.RowPadV),
            placeholder = "NEW PLAYLIST NAME",
        )
        ActionRow(SheetAction("CREATE AND ADD", OrbitIcons.Plus, enabled = name.isNotBlank()) {
            val n = name.trim()
            env.collections.createPlaylist(n) { created ->
                env.collections.addToPlaylist(created.id, trackIds) { onDone(created.name) }
            }
        })
        LazyColumn {
            items(snapshot?.playlists.orEmpty(), key = { it.id }, contentType = { "playlist" }) { p ->
                val count = p.trackIds.size
                Row(
                    Modifier.fillMaxWidth().pressable(pressedScale = 0.985f) {
                        env.collections.addToPlaylist(p.id, trackIds) { onDone(p.name) }
                    }.padding(horizontal = Space.Gutter, vertical = Space.M),
                    horizontalArrangement = Arrangement.spacedBy(Dimens.RowGap),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Cover(p.coverPath, size = 36.dp)
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                        Text(p.name, color = Orbit.Text, style = OrbitType.RowTitle, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            pluralCount(count, "TRACK", "TRACKS"),
                            color = Orbit.TextDim,
                            style = OrbitType.Badge,
                        )
                    }
                }
            }
        }
    }
}
