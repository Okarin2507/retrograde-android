package dev.okarin.rustygrade.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import dev.okarin.rustygrade.ui.components.OrbitTextField
import dev.okarin.rustygrade.ui.components.SectionTitle
import dev.okarin.rustygrade.ui.components.digitsOnly
import dev.okarin.rustygrade.ui.theme.Space
import uniffi.rustygrade_ffi.AlbumSummary
import uniffi.rustygrade_ffi.TagEditInfo
import uniffi.rustygrade_ffi.TrackInfo

/** Tracks to edit, the album they belong to (album mode only), cover and single-track lyrics. */
@Stable
class TagLoad(
    val tracks: List<TrackInfo>,
    val album: AlbumSummary?,
    val coverPath: String?,
    val lyrics: String = "",
)

/**
 * One editable tag value. [initial] is what the tracks agree on (empty when they differ or the
 * value is unknown); the field counts as changed once the trimmed text differs from it.
 */
@Stable
class TagField(val initial: String, val placeholder: String) {
    var text by mutableStateOf(initial)
    val changed: Boolean get() = text.trim() != initial
}

/** The editable fields of the tag editor, prefilled from the tracks being edited. */
@Stable
class TagForm(tracks: List<TrackInfo>, album: AlbumSummary?, existingLyrics: String = "") {
    val title = commonText(tracks.map { it.title })
    val artist = commonText(tracks.map { it.artist })
    val albumName = commonText(tracks.map { it.album })

    // The track list carries no year or album artist: in album mode they come from the album,
    // for a single track they are unknown and only sent when the user types something.
    val albumArtist = if (album != null) TagField(album.artist, "") else TagField("", UNKNOWN)
    val year = if (album != null) TagField(if (album.year > 0) album.year.toString() else "", "") else TagField("", UNKNOWN)
    val track = commonNumber(tracks.map { it.trackNumber })
    val disc = commonNumber(tracks.map { it.discNumber })
    val lyrics = TagField(existingLyrics.trim(), "NO LYRICS")

    /** Builds the edit from the changed fields only; everything else stays null (untouched). */
    fun toEdit(albumMode: Boolean, coverImagePath: String?, removeCover: Boolean): TagEditInfo = TagEditInfo(
        title = if (!albumMode && title.changed && title.text.isNotBlank()) title.text.trim() else null,
        artist = if (artist.changed) artist.text.trim() else null,
        album = if (albumName.changed) albumName.text.trim() else null,
        albumArtist = if (albumArtist.changed) albumArtist.text.trim() else null,
        year = number(year),
        track = if (albumMode) null else number(track),
        disc = number(disc),
        coverImagePath = coverImagePath,
        removeCover = removeCover && coverImagePath == null,
        lyrics = if (!albumMode && lyrics.changed) lyrics.text.trim() else null,
    )

    private fun number(field: TagField): UInt? =
        if (field.changed) field.text.trim().toUIntOrNull() ?: 0u else null

    private companion object {
        const val UNKNOWN = "UNCHANGED"

        fun commonText(values: List<String>): TagField {
            val first = values.firstOrNull().orEmpty()
            return if (values.any { it != first }) TagField("", "MIXED") else TagField(first, "")
        }

        fun commonNumber(values: List<Int>): TagField {
            val first = values.firstOrNull() ?: 0
            return when {
                values.any { it != first } -> TagField("", "MIXED")
                first > 0 -> TagField(first.toString(), "")
                else -> TagField("", "")
            }
        }
    }
}

/** True when [edit] changes at least one tag, lyrics or the cover. */
fun TagEditInfo.hasChanges(): Boolean =
    title != null || artist != null || album != null || albumArtist != null ||
        year != null || track != null || disc != null || coverImagePath != null || removeCover || lyrics != null

/** Year / track / disc fields: at most four digits. Stateless, so one instance serves every field. */
private val FourDigits = digitsOnly(4)

/** A tracked [label] above an [OrbitTextField] bound to [field]; [numeric] filters to digits. */
@Composable
internal fun TagFieldRow(label: String, field: TagField, modifier: Modifier = Modifier, numeric: Boolean = false) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(Space.Compact)) {
        SectionTitle(label)
        OrbitTextField(
            value = field.text,
            onValueChange = { field.text = it },
            inputTransformation = if (numeric) FourDigits else null,
            modifier = Modifier.fillMaxWidth(),
            placeholder = field.placeholder,
            keyboardType = if (numeric) KeyboardType.Number else KeyboardType.Text,
        )
    }
}
