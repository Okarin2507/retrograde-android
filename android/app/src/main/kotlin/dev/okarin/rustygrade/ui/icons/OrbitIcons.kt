package dev.okarin.rustygrade.ui.icons

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.dp

/**
 * The Lucide icons of the desktop app (`reference/icon_paths.rs`, 24x24 grid) as `ImageVector`s.
 * Tint them at the call site (`Icon(OrbitIcons.Play, ..., tint = ...)`): the vectors are white.
 */
object OrbitIcons {
    /** Solid shapes in the original; everything else is a 2 px outline. */
    private val FILLED = setOf("play", "pause", "skipback", "skipforward", "heartfilled")

    private fun build(name: String, path: String): ImageVector {
        val nodes = PathParser().parsePathString(path).toNodes()
        val filled = name in FILLED
        return ImageVector.Builder(name, 24.dp, 24.dp, 24f, 24f)
            .addPath(
                pathData = nodes,
                fill = if (filled) SolidColor(Color.White) else null,
                stroke = SolidColor(Color.White),
                strokeLineWidth = 2f,
                strokeLineCap = StrokeCap.Round,
                strokeLineJoin = StrokeJoin.Round,
            )
            .build()
    }

    val Chevrondown: ImageVector by lazy { build("chevrondown", "M6,9 L12,15 L18,9") }
    val Terminal: ImageVector by lazy { build("terminal", "M4,17 L10,11 L4,5 M12,19 H20") }
    val X: ImageVector by lazy { build("x", "M18,6 L6,18 M6,6 L18,18") }
    val Search: ImageVector by lazy { build("search", "M19,11 A8,8 0 1 1 3,11 A8,8 0 1 1 19,11 Z M21,21 L16.7,16.7") }
    val Folderplus: ImageVector by lazy { build("folderplus", "M12,10 V16 M9,13 H15 M20,20 A2,2 0 0 0 22,18 V8 A2,2 0 0 0 20,6 H12.1 A2,2 0 0 1 10.41,5.1 L9.6,3.9 A2,2 0 0 0 7.93,3 H4 A2,2 0 0 0 2,5 V18 A2,2 0 0 0 4,20 Z") }
    val Heart: ImageVector by lazy { build("heart", "M19,14 C20.49,12.54 22,10.79 22,8.5 A5.5,5.5 0 0 0 16.5,3 C14.74,3 13.5,3.5 12,5 C10.5,3.5 9.26,3 7.5,3 A5.5,5.5 0 0 0 2,8.5 C2,10.8 3.5,12.55 5,14 L12,21 Z") }
    val Heartfilled: ImageVector by lazy { build("heartfilled", "M19,14 C20.49,12.54 22,10.79 22,8.5 A5.5,5.5 0 0 0 16.5,3 C14.74,3 13.5,3.5 12,5 C10.5,3.5 9.26,3 7.5,3 A5.5,5.5 0 0 0 2,8.5 C2,10.8 3.5,12.55 5,14 L12,21 Z") }
    val Plus: ImageVector by lazy { build("plus", "M5,12 H19 M12,5 V19") }
    val Arrowupdown: ImageVector by lazy { build("arrowupdown", "M21,16 L17,20 L13,16 M17,20 V4 M3,8 L7,4 L11,8 M7,4 V20") }
    val Checksquare: ImageVector by lazy { build("checksquare", "M21,10.5 V19 A2,2 0 0 1 19,21 H5 A2,2 0 0 1 3,19 V5 A2,2 0 0 1 5,3 H17.5 M9,11 L12,14 L22,4") }
    val Play: ImageVector by lazy { build("play", "M6,3 L20,12 L6,21 Z") }
    val Pause: ImageVector by lazy { build("pause", "M6,4 H10 V20 H6 Z M14,4 H18 V20 H14 Z") }
    val Skipback: ImageVector by lazy { build("skipback", "M19,20 L9,12 L19,4 Z M5,19 V5") }
    val Skipforward: ImageVector by lazy { build("skipforward", "M5,4 L15,12 L5,20 Z M19,5 V19") }
    val Shuffle: ImageVector by lazy { build("shuffle", "M2,18 H3.4 C4.7,18 5.9,17.4 6.7,16.3 L12.8,7.7 C13.5,6.6 14.8,6 16.1,6 H22 M18,2 L22,6 L18,10 M2,6 H3.9 C5.4,6 6.8,6.9 7.5,8.2 M22,18 H16.1 C14.8,18 13.5,17.3 12.8,16.2 L12.3,15.4 M18,14 L22,18 L18,22") }
    val Repeat: ImageVector by lazy { build("repeat", "M17,2 L21,6 L17,10 M3,11 V10 A4,4 0 0 1 7,6 H21 M7,22 L3,18 L7,14 M21,13 V14 A4,4 0 0 1 17,18 H3") }
    val Repeat1: ImageVector by lazy { build("repeat1", "M17,2 L21,6 L17,10 M3,11 V10 A4,4 0 0 1 7,6 H21 M7,22 L3,18 L7,14 M21,13 V14 A4,4 0 0 1 17,18 H3 M11,10 H12 V14") }
    val Volume2: ImageVector by lazy { build("volume2", "M11,5 L6,9 H2 V15 H6 L11,19 Z M15.54,8.46 A5,5 0 0 1 15.54,15.53 M19.07,4.93 A10,10 0 0 1 19.07,19.07") }
    val Volumex: ImageVector by lazy { build("volumex", "M11,5 L6,9 H2 V15 H6 L11,19 Z M22,9 L16,15 M16,9 L22,15") }
    val Listmusic: ImageVector by lazy { build("listmusic", "M21,15 V6 M18.5,18 A2.5,2.5 0 1 0 18.5,13 A2.5,2.5 0 0 0 18.5,18 Z M12,12 H3 M16,6 H3 M12,18 H3") }
    val Playnext: ImageVector by lazy { build("playnext", "M16,12 H3 M16,18 H3 M10,6 H3 M21,18 V8 A2,2 0 0 0 19,6 H14 M16,8 L14,6 L16,4") }
    val Sliders: ImageVector by lazy { build("sliders", "M4,21 V14 M4,10 V3 M12,21 V12 M12,8 V3 M20,21 V16 M20,12 V3 M2,14 H6 M10,8 H14 M18,16 H22") }
    val Grip: ImageVector by lazy { build("grip", "M9,6 H9.01 M9,12 H9.01 M9,18 H9.01 M15,6 H15.01 M15,12 H15.01 M15,18 H15.01") }
    val Arrowleft: ImageVector by lazy { build("arrowleft", "M12,19 L5,12 L12,5 M19,12 H5") }
    val Disc: ImageVector by lazy { build("disc", "M22,12 A10,10 0 1 1 2,12 A10,10 0 1 1 22,12 Z M14,12 A2,2 0 1 1 10,12 A2,2 0 1 1 14,12 Z") }
    val Upload: ImageVector by lazy { build("upload", "M21,15 V19 A2,2 0 0 1 19,21 H5 A2,2 0 0 1 3,19 V15 M17,8 L12,3 L7,8 M12,3 V15") }
    val Download: ImageVector by lazy { build("download", "M21,15 V19 A2,2 0 0 1 19,21 H5 A2,2 0 0 1 3,19 V15 M7,10 L12,15 L17,10 M12,15 V3") }
    val Refresh: ImageVector by lazy { build("refresh", "M3,12 A9,9 0 0 1 18.36,5.64 L21,8 M21,3 V8 H16 M21,12 A9,9 0 0 1 5.64,18.36 L3,16 M8,16 H3 V21") }
    val Image: ImageVector by lazy { build("image", "M5,3 H19 A2,2 0 0 1 21,5 V19 A2,2 0 0 1 19,21 H5 A2,2 0 0 1 3,19 V5 A2,2 0 0 1 5,3 Z M11,9 A2,2 0 1 1 7,9 A2,2 0 1 1 11,9 Z M21,15 L17.914,11.914 A2,2 0 0 0 15.086,11.914 L6,21") }
    val Trash: ImageVector by lazy { build("trash", "M3,6 H21 M19,6 V20 C19,21 18,22 17,22 H7 C6,22 5,21 5,20 V6 M8,6 V4 C8,3 9,2 10,2 H14 C15,2 16,3 16,4 V6") }
    val Pencil: ImageVector by lazy { build("pencil", "M21.17,6.81 A2.82,2.82 0 0 0 17.19,2.83 L3.84,16.17 A2,2 0 0 0 3.34,17 L2.02,21.35 A0.5,0.5 0 0 0 2.64,21.97 L7,20.66 A2,2 0 0 0 7.83,20.16 Z M15,5 L19,9") }
    val Filter: ImageVector by lazy { build("filter", "M3,6 H21 M7,12 H17 M10,18 H14") }
    val User: ImageVector by lazy { build("user", "M19,21 V19 A4,4 0 0 0 15,15 H9 A4,4 0 0 0 5,19 V21 M16,7 A4,4 0 1 1 8,7 A4,4 0 1 1 16,7 Z") }
    val More: ImageVector by lazy { build("more", "M13,12 A1,1 0 1 1 11,12 A1,1 0 1 1 13,12 Z M20,12 A1,1 0 1 1 18,12 A1,1 0 1 1 20,12 Z M6,12 A1,1 0 1 1 4,12 A1,1 0 1 1 6,12 Z") }
}
