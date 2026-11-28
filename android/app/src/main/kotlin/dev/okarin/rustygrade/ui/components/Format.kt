package dev.okarin.rustygrade.ui.components

import java.util.Locale

/** `mm:ss` (or `h:mm:ss`) for a duration in seconds. */
fun formatDuration(seconds: Double): String {
    val total = seconds.toLong().coerceAtLeast(0)
    val h = total / 3600
    val m = (total % 3600) / 60
    val s = total % 60
    return if (h > 0) "%d:%02d:%02d".format(Locale.US, h, m, s) else "%d:%02d".format(Locale.US, m, s)
}

/** Formats a singular or plural count label, e.g. `1 TRACK` vs `3 TRACKS`. */
fun pluralCount(n: Int, one: String, many: String): String = if (n == 1) "$n $one" else "$n $many"

/** UInt overload of [pluralCount]. */
fun pluralCount(n: UInt, one: String, many: String): String = if (n == 1u) "$n $one" else "$n $many"

/** Digits only, at most [maxLength] of them: rejected typing never reaches the field. */
fun digitsOnly(maxLength: Int): androidx.compose.foundation.text.input.InputTransformation =
    androidx.compose.foundation.text.input.InputTransformation {
        val now = asCharSequence().toString()
        val filtered = now.filter(Char::isDigit).take(maxLength)
        if (filtered != now) replace(0, length, filtered)
    }
