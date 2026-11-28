package dev.okarin.rustygrade.ui.screens

import androidx.compose.runtime.saveable.Saver
import uniffi.rustygrade_ffi.SmartPlaylistInfo
import uniffi.rustygrade_ffi.SmartRuleInfo

/**
 * Keeps the smart-playlist editor's unsaved draft across rotation and process death. The record is
 * flattened to a `Bundle`-safe list (`limit` as a `Long`, rules as a flat list of strings) because
 * the generated record is not `Parcelable`.
 */
internal val SmartDraftSaver: Saver<SmartPlaylistInfo?, ArrayList<Any>> = Saver(
    save = { draft -> draft?.let(::encodeSmartDraft) },
    restore = { saved -> decodeSmartDraft(saved) },
)

internal fun encodeSmartDraft(draft: SmartPlaylistInfo): ArrayList<Any> = arrayListOf(
    draft.id,
    draft.name,
    draft.matchAll,
    draft.sort,
    draft.limit.toLong(),
    ArrayList<String>(draft.rules.size * 3).also { flat ->
        draft.rules.forEach { flat.add(it.field); flat.add(it.op); flat.add(it.value) }
    },
)

/** The inverse of [encodeSmartDraft]; `null` for anything that is not a well-formed draft. */
internal fun decodeSmartDraft(saved: List<*>): SmartPlaylistInfo? {
    if (saved.size != 6) return null
    val id = saved[0] as? Long ?: return null
    val name = saved[1] as? String ?: return null
    val matchAll = saved[2] as? Boolean ?: return null
    val sort = saved[3] as? String ?: return null
    val limit = (saved[4] as? Long)?.takeIf { it in 0..UInt.MAX_VALUE.toLong() }?.toUInt() ?: return null
    val flat = (saved[5] as? List<*>)?.map { it as? String ?: return null } ?: return null
    if (flat.size % 3 != 0) return null
    val rules = flat.chunked(3).map { (field, op, value) -> SmartRuleInfo(field, op, value) }
    return SmartPlaylistInfo(id = id, name = name, matchAll = matchAll, rules = rules, sort = sort, limit = limit)
}
