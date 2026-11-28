package dev.okarin.rustygrade.media

/** The slice of a player's playlist the reconciler needs; ExoPlayer implements it in the coordinator. */
interface WindowList<T> {
    val count: Int
    val currentIndex: Int
    fun idAt(index: Int): String
    fun currentId(): String? = if (count > 0) idAt(currentIndex) else null

    /** Replaces everything, starts at [startIndex] / [startMs] and prepares. */
    fun setAll(items: List<T>, startIndex: Int, startMs: Long)
    fun insert(index: Int, item: T)
    fun append(item: T)
    fun remove(fromIndex: Int, toIndexExclusive: Int)
}

/** What the player's window must look like: `[previous?, current, next?]`. */
class WindowSpec<T>(val previous: T?, val current: T, val next: T?)

/**
 * Makes a player's playlist match a [WindowSpec] with as few edits as possible, never touching the
 * item that is playing (so playback is not interrupted and gapless transitions stay gapless).
 *
 * Kept free of Android/Media3 types so the index arithmetic can be unit-tested with a fake list.
 */
class WindowReconciler<T>(private val list: WindowList<T>, private val idOf: (T) -> String) {

    /** Returns `true` when the whole playlist was replaced (a reload), `false` for incremental edits. */
    fun reconcile(spec: WindowSpec<T>, forceReload: Boolean, startMs: Long = 0L): Boolean {
        val currentId = idOf(spec.current)
        val holdsCurrent = !forceReload && list.count > 0 && list.currentId() == currentId
        if (!holdsCurrent) {
            val items = listOfNotNull(spec.previous, spec.current, spec.next)
            list.setAll(items, if (spec.previous != null) 1 else 0, startMs)
            return true
        }

        // Before the current item: exactly `previous` (or nothing).
        var ci = list.currentIndex
        val wantPrev = spec.previous?.let(idOf)
        val havePrev = if (ci > 0) list.idAt(ci - 1) else null
        if (ci > 1 || havePrev != wantPrev) {
            if (ci > 0) list.remove(0, ci)
            spec.previous?.let { list.insert(0, it) }
            ci = list.currentIndex
        }

        // After the current item: exactly `next` (or nothing); touched only when its id changed.
        val wantNext = spec.next?.let(idOf)
        val after = list.count - 1 - ci
        val haveNext = if (after == 1) list.idAt(ci + 1) else null
        if (after > 1 || haveNext != wantNext) {
            if (after > 0) list.remove(ci + 1, list.count)
            spec.next?.let { list.append(it) }
        }
        return false
    }
}
