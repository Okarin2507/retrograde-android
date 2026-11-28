use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::items::Timestamp;
use lofty::tag::{Accessor, ItemKey, Tag};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TagWriteError {
    #[error("Failed to open or save audio file: {0}")]
    Lofty(String),
    #[error("Failed to read cover image: {0}")]
    Io(#[from] std::io::Error),
    #[error("Unsupported audio file (no tag container available)")]
    NoTag,
    #[error("Cover image rejected: {0}")]
    BadCover(String),
}

/// Largest cover image embedded into a tag: bigger ones bloat every file and every scan of it.
pub const MAX_TAG_COVER_BYTES: u64 = 20 * 1024 * 1024;

use crate::models::ReplayGain;

#[derive(Debug, Clone, Default)]
pub struct TagEdit {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<u32>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub cover_image: Option<PathBuf>,
    pub remove_cover: bool,
    pub lyrics: Option<String>,
    pub replay_gain: Option<ReplayGain>,
}

pub struct TagWriter;

impl TagWriter {
    /// Writes `edit` into the file at `path`, crash-safely: the change is made on a copy beside the
    /// file and renamed over it, so a failure (or a panic inside lofty, which is caught) leaves the
    /// original untouched.
    pub fn write(path: &Path, edit: &TagEdit) -> Result<(), TagWriteError> {
        // Validate and read the cover first: nothing is copied if it is going to be refused.
        let cover = edit.cover_image.as_deref().map(load_cover).transpose()?;

        let dest = fs::canonicalize(path).map_err(|e| TagWriteError::Lofty(e.to_string()))?;
        // Same directory (so the rename never crosses a filesystem) and same extension (lofty
        // recognises the format by it).
        let tmp = crate::fsutil::unique_sibling(&dest);
        fs::copy(&dest, &tmp).map_err(|e| TagWriteError::Lofty(e.to_string()))?;

        let outcome = catch_unwind(AssertUnwindSafe(|| Self::edit_in_place(&tmp, edit, cover)))
            .unwrap_or_else(|_| Err(TagWriteError::Lofty("the tag library failed on this file".to_owned())));
        match outcome.and_then(|()| fs::rename(&tmp, &dest).map_err(|e| TagWriteError::Lofty(e.to_string()))) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    fn edit_in_place(path: &Path, edit: &TagEdit, cover: Option<Picture>) -> Result<(), TagWriteError> {
        let mut tagged = Probe::open(path)
            .map_err(|e| TagWriteError::Lofty(e.to_string()))?
            .read()
            .map_err(|e| TagWriteError::Lofty(e.to_string()))?;
        let tag_type = tagged.primary_tag_type();
        if tagged.primary_tag().is_none() {
            tagged.insert_tag(Tag::new(tag_type));
        }
        let tag = tagged.primary_tag_mut().ok_or(TagWriteError::NoTag)?;

        if let Some(title) = &edit.title {
            let trimmed = title.trim();
            if trimmed.is_empty() {
                tag.remove_title();
            } else {
                tag.set_title(trimmed.to_owned());
            }
        }
        if let Some(artist) = &edit.artist {
            let trimmed = artist.trim();
            if trimmed.is_empty() {
                tag.remove_artist();
            } else {
                tag.set_artist(trimmed.to_owned());
            }
        }
        if let Some(album) = &edit.album {
            let trimmed = album.trim();
            if trimmed.is_empty() {
                tag.remove_album();
            } else {
                tag.set_album(trimmed.to_owned());
            }
        }
        if let Some(album_artist) = &edit.album_artist {
            let trimmed = album_artist.trim();
            if trimmed.is_empty() {
                tag.remove_key(ItemKey::AlbumArtist);
            } else {
                tag.insert_text(ItemKey::AlbumArtist, trimmed.to_owned());
            }
        }
        if let Some(year) = edit.year {
            if year == 0 {
                tag.remove_date();
            } else {
                tag.set_date(Timestamp {
                    year: year.min(u16::MAX as u32) as u16,
                    ..Timestamp::default()
                });
            }
        }
        if let Some(track) = edit.track {
            if track == 0 {
                tag.remove_track();
            } else {
                tag.set_track(track);
            }
        }
        if let Some(disc) = edit.disc {
            if disc == 0 {
                tag.remove_disk();
            } else {
                tag.set_disk(disc);
            }
        }
        if let Some(mut picture) = cover {
            picture.set_pic_type(PictureType::CoverFront);
            picture.set_description(Some("Cover".to_owned()));
            while !tag.pictures().is_empty() {
                tag.remove_picture(0);
            }
            tag.push_picture(picture);
        } else if edit.remove_cover {
            while !tag.pictures().is_empty() {
                tag.remove_picture(0);
            }
        }
        if let Some(lyrics) = &edit.lyrics {
            let trimmed = lyrics.trim();
            if trimmed.is_empty() {
                tag.remove_key(ItemKey::Lyrics);
            } else {
                tag.insert_text(ItemKey::Lyrics, trimmed.to_owned());
            }
        }
        if let Some(rg) = &edit.replay_gain {
            if let Some(g) = rg.track_gain_db.filter(|v| v.is_finite()) {
                tag.insert_text(ItemKey::ReplayGainTrackGain, format!("{g:+.2} dB"));
            }
            if let Some(p) = rg.track_peak.filter(|v| v.is_finite() && *v >= 0.0) {
                tag.insert_text(ItemKey::ReplayGainTrackPeak, format!("{p:.6}"));
            }
            if let Some(g) = rg.album_gain_db.filter(|v| v.is_finite()) {
                tag.insert_text(ItemKey::ReplayGainAlbumGain, format!("{g:+.2} dB"));
            }
            if let Some(p) = rg.album_peak.filter(|v| v.is_finite() && *v >= 0.0) {
                tag.insert_text(ItemKey::ReplayGainAlbumPeak, format!("{p:.6}"));
            }
        }

        tagged
            .save_to_path(path, WriteOptions::default())
            .map_err(|e| TagWriteError::Lofty(e.to_string()))?;
        Ok(())
    }
}

/// Reads a cover image for embedding, refusing anything that is not a sane, reasonably small image.
fn load_cover(image_path: &Path) -> Result<Picture, TagWriteError> {
    let meta = fs::metadata(image_path)?;
    if !meta.is_file() {
        return Err(TagWriteError::BadCover("not a file".to_owned()));
    }
    if meta.len() > MAX_TAG_COVER_BYTES {
        return Err(TagWriteError::BadCover(format!("larger than {} MiB", MAX_TAG_COVER_BYTES / (1024 * 1024))));
    }
    if !crate::covers::ImageLimits::is_sane_path(image_path) {
        return Err(TagWriteError::BadCover("unreadable, unsupported or too many pixels".to_owned()));
    }
    // Read at most the cap (+1 to notice growth since the size check), never an unbounded stream.
    let mut bytes = Vec::new();
    File::open(image_path)?.take(MAX_TAG_COVER_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_TAG_COVER_BYTES {
        return Err(TagWriteError::BadCover("larger than the size limit".to_owned()));
    }
    catch_unwind(AssertUnwindSafe(|| Picture::from_reader(&mut Cursor::new(bytes))))
        .map_err(|_| TagWriteError::Lofty("the tag library failed on this image".to_owned()))?
        .map_err(|e| TagWriteError::Lofty(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flac_in(name: &str) -> (crate::testing::TestEnv, PathBuf) {
        let env = crate::testing::TestEnv::new(name);
        let song = env.music.join("song.flac");
        // Some trailing "audio": lofty rewrites a stream-less FLAC unreliably when a tag grows or shrinks.
        let mut bytes = crate::testing::minimal_flac_bytes();
        bytes.extend(std::iter::repeat_n(0xFFu8, 4096));
        fs::write(&song, bytes).unwrap();
        let seed = TagEdit {
            title: Some("Old".into()),
            artist: Some("Old Artist".into()),
            album: Some("Old Album".into()),
            track: Some(3),
            ..Default::default()
        };
        TagWriter::write(&song, &seed).unwrap();
        (env, song)
    }

    fn read_tag(path: &Path) -> lofty::tag::Tag {
        Probe::open(path).unwrap().read().unwrap().primary_tag().cloned().expect("tag")
    }

    #[test]
    fn empty_strings_and_zero_numbers_clear_fields() {
        let (_env, song) = flac_in("tag-clear");
        TagWriter::write(
            &song,
            &TagEdit { album_artist: Some("AA".into()), year: Some(1999), disc: Some(2), ..Default::default() },
        )
        .unwrap();
        let t = read_tag(&song);
        assert_eq!((t.title().as_deref(), t.artist().as_deref()), (Some("Old"), Some("Old Artist")));
        assert_eq!(t.date().map(|d| d.year), Some(1999));
        assert_eq!(t.disk(), Some(2));
        TagWriter::write(
            &song,
            &TagEdit {
                title: Some("  ".into()),
                artist: Some(String::new()),
                album: Some(" New ".into()),
                album_artist: Some(String::new()),
                year: Some(0),
                track: Some(0),
                disc: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        let t = read_tag(&song);
        assert_eq!(t.title(), None);
        assert_eq!(t.artist(), None);
        assert_eq!(t.album().as_deref(), Some("New"));
        assert_eq!(t.get_string(ItemKey::AlbumArtist), None);
        assert_eq!((t.date(), t.track(), t.disk()), (None, None, None));
    }

    #[test]
    fn cover_can_be_embedded_and_removed() {
        let (env, song) = flac_in("tag-cover");
        let img = env.music.join("cover.png");
        crate::testing::write_png(&img, [10, 20, 30]);
        TagWriter::write(&song, &TagEdit { cover_image: Some(img), ..Default::default() }).unwrap();
        assert_eq!(read_tag(&song).pictures().len(), 1);
        TagWriter::write(&song, &TagEdit { remove_cover: true, ..Default::default() }).unwrap();
        assert!(read_tag(&song).pictures().is_empty());
    }

    #[test]
    fn a_huge_or_bogus_cover_is_refused_and_the_file_is_untouched() {
        let (env, song) = flac_in("tag-bad-cover");
        let before = fs::read(&song).unwrap();
        let huge = env.music.join("huge.png");
        let f = File::create(&huge).unwrap();
        f.set_len(MAX_TAG_COVER_BYTES + 1).unwrap();
        let junk = env.music.join("junk.png");
        fs::write(&junk, b"definitely not an image").unwrap();
        for cover in [huge, junk, env.music.join("missing.png"), env.music.clone()] {
            let edit = TagEdit { title: Some("Changed".into()), cover_image: Some(cover), ..Default::default() };
            assert!(TagWriter::write(&song, &edit).is_err());
        }
        assert_eq!(fs::read(&song).unwrap(), before, "a refused edit changes nothing");
        let strays = fs::read_dir(&env.music).unwrap().filter(|e| {
            e.as_ref().unwrap().file_name().to_string_lossy().contains(".tmp")
        });
        assert_eq!(strays.count(), 0, "no temp copies are left behind");
    }

    #[test]
    fn a_failed_write_keeps_the_original_and_cleans_up() {
        let env = crate::testing::TestEnv::new("tag-garbage");
        let bad = env.music.join("bad.flac");
        fs::write(&bad, b"this is not audio at all").unwrap();
        assert!(TagWriter::write(&bad, &TagEdit { title: Some("x".into()), ..Default::default() }).is_err());
        assert_eq!(fs::read(&bad).unwrap(), b"this is not audio at all");
        assert_eq!(fs::read_dir(&env.music).unwrap().count(), 1);
        // A missing file is an error too, not a panic.
        assert!(TagWriter::write(&env.music.join("nope.flac"), &TagEdit::default()).is_err());
    }

    #[test]
    fn write_goes_through_a_symlink_to_the_real_file() {
        let (env, song) = flac_in("tag-symlink");
        let link = env.music.join("link.flac");
        std::os::unix::fs::symlink(&song, &link).unwrap();
        TagWriter::write(&link, &TagEdit { title: Some("Via link".into()), ..Default::default() }).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link survives");
        assert_eq!(read_tag(&song).title().as_deref(), Some("Via link"));
    }
}
