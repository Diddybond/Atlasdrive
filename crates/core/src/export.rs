//! Gathering photographs out of the archive.
//!
//! Two exports, both strictly one-directional: they *read* originals and write
//! only into a destination the user chose.
//!
//!   * [`copy_photos`] — collect a person's photographs into a folder, so they
//!     can be handed to someone or opened in another application.
//!   * [`write_xmp_sidecars`] — write `.xmp` files alongside originals carrying
//!     names and keywords, which Bridge, Lightroom and Capture One all read.
//!
//! ## The line these must not cross
//!
//! `copy_photos` never moves, never deletes, and never writes to a source
//! drive. `write_xmp_sidecars` *does* write to the drive, which is why it is
//! opt-in and explicit in exactly the way `--write-manifest` is (D-005): it
//! creates a new file next to the original and never opens the original for
//! writing. Nothing here can alter a photograph.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Outcome of gathering photographs into a folder.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportSummary {
    pub copied: u64,
    /// Already present in the destination with the same size — left alone.
    pub skipped_existing: u64,
    /// On a drive that is not currently connected.
    pub skipped_offline: u64,
    /// Present in the catalogue but not found on the connected drive.
    pub missing: u64,
    /// Drive numbers the user needs to connect to finish the job.
    pub drives_to_connect: Vec<i64>,
    pub destination: String,
}

impl ExportSummary {
    /// One sentence a person can act on.
    pub fn summary(&self) -> String {
        let mut s = format!("Copied {} photograph(s) to {}.", self.copied, self.destination);
        if self.skipped_existing > 0 {
            s.push_str(&format!(" {} already there.", self.skipped_existing));
        }
        if self.skipped_offline > 0 {
            let mut drives: Vec<String> =
                self.drives_to_connect.iter().map(|d| d.to_string()).collect();
            drives.sort();
            s.push_str(&format!(
                " {} more are on Drive {} — connect and run this again to get them.",
                self.skipped_offline,
                drives.join(", ")
            ));
        }
        if self.missing > 0 {
            s.push_str(&format!(" {} could not be found on the drive.", self.missing));
        }
        s
    }
}

/// Copy the given catalogued files into `destination`.
///
/// Filenames are prefixed with the drive number, because the same filename on
/// two drives is common in a photographer's archive and silently overwriting one
/// with the other would be data loss in a feature that is supposed to be safe.
pub fn copy_photos(
    conn: &Connection,
    file_ids: &[String],
    destination: &Path,
) -> Result<ExportSummary> {
    if file_ids.is_empty() {
        return Err(Error::InvalidArgs("nothing selected to copy".into()));
    }
    std::fs::create_dir_all(destination)?;

    let mut summary = ExportSummary {
        destination: destination.display().to_string(),
        ..Default::default()
    };
    let mut to_connect: std::collections::BTreeSet<i64> = Default::default();

    for file_id in file_ids {
        let row = conn.query_row(
            "SELECT f.filename, d.drive_number, d.status
               FROM files f JOIN drives d ON d.id = f.drive_id
              WHERE f.id = ?1",
            [file_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        );
        let Ok((filename, drive_number, status)) = row else {
            summary.missing += 1;
            continue;
        };

        let Some(source) = crate::search::resolve_original(conn, file_id)? else {
            // Not reachable: either the drive is unplugged, or the file is gone.
            if status == "online" {
                summary.missing += 1;
            } else {
                summary.skipped_offline += 1;
                to_connect.insert(drive_number);
            }
            continue;
        };

        let target = destination.join(format!("drive{drive_number:02}_{filename}"));
        if let Ok(existing) = std::fs::metadata(&target) {
            let same_size = std::fs::metadata(&source)
                .map(|m| m.len() == existing.len())
                .unwrap_or(false);
            if same_size {
                summary.skipped_existing += 1;
                continue;
            }
        }
        // `copy` reads the original and writes the destination. The original is
        // never opened for writing, moved or removed.
        std::fs::copy(&source, &target)?;
        summary.copied += 1;
    }

    summary.drives_to_connect = to_connect.into_iter().collect();
    Ok(summary)
}

/// Everything a sidecar says about one photograph.
#[derive(Debug, Clone)]
pub struct SidecarSubject {
    pub file_id: String,
    /// People confirmed in this photograph.
    pub people: Vec<String>,
    /// Automatic subject keywords.
    pub keywords: Vec<String>,
    pub description: Option<String>,
}

/// Gather what a sidecar would say for each catalogued file.
pub fn sidecar_subjects(conn: &Connection, file_ids: &[String]) -> Result<Vec<SidecarSubject>> {
    let mut out = Vec::new();
    for file_id in file_ids {
        let mut people_stmt = conn.prepare(
            "SELECT DISTINCT p.display_name
               FROM faces f
               JOIN face_clusters c ON c.id = f.cluster_id
               JOIN people p        ON p.id = c.person_id
              WHERE f.file_id = ?1 AND c.status = 'confirmed'
                AND f.is_false_detection = 0",
        )?;
        let people: Vec<String> = people_stmt
            .query_map([file_id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut tag_stmt = conn.prepare(
            "SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
              WHERE ft.file_id = ?1 AND t.tag_type <> 'person'
              ORDER BY ft.confidence DESC LIMIT 20",
        )?;
        let keywords: Vec<String> = tag_stmt
            .query_map([file_id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let description: Option<String> = conn
            .query_row(
                "SELECT description FROM scene_analysis WHERE file_id = ?1",
                [file_id],
                |r| r.get(0),
            )
            .ok();

        out.push(SidecarSubject { file_id: file_id.clone(), people, keywords, description });
    }
    Ok(out)
}

/// Render an XMP sidecar document.
///
/// Deliberately minimal and standards-plain: `dc:subject` for keywords (which is
/// what Bridge, Lightroom and Capture One all read as keywords) and
/// `dc:description`. People are written as keywords too, since that is the only
/// field every one of those applications agrees on.
pub fn render_xmp(subject: &SidecarSubject) -> String {
    let mut keywords: Vec<String> = subject.people.clone();
    keywords.extend(subject.keywords.iter().cloned());
    keywords.dedup();

    let items: String = keywords
        .iter()
        .map(|k| format!("     <rdf:li>{}</rdf:li>\n", escape_xml(k)))
        .collect();
    let description = subject
        .description
        .as_deref()
        .map(|d| {
            format!(
                "   <dc:description>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n    </rdf:Alt>\n   </dc:description>\n",
                escape_xml(d)
            )
        })
        .unwrap_or_default();

    format!(
        r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="AtlasDrive">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/">
   <dc:subject>
    <rdf:Bag>
{items}    </rdf:Bag>
   </dc:subject>
{description}  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
"#
    )
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Result of writing sidecars.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SidecarSummary {
    pub written: u64,
    pub skipped_offline: u64,
    pub skipped_nothing_to_say: u64,
    /// A sidecar already existed and was left completely untouched.
    pub skipped_existing: u64,
    pub paths: Vec<String>,
}

impl SidecarSummary {
    pub fn summary(&self) -> String {
        let mut s = format!("Wrote {} new sidecar(s).", self.written);
        if self.skipped_existing > 0 {
            s.push_str(&format!(
                " {} photograph(s) already had a sidecar — those files were not touched.",
                self.skipped_existing
            ));
        }
        if self.skipped_offline > 0 {
            s.push_str(&format!(" {} are on a disconnected drive.", self.skipped_offline));
        }
        s
    }
}

/// Create a file only if nothing is there, and write it.
///
/// **This is the safeguard that protects existing edits.** A photographer's
/// `.xmp` beside a RAW is not metadata AtlasDrive owns — it holds Camera Raw
/// develop settings (`crs:Blacks`, `crs:Clarity`, `crs:ColorGrade`, and hundreds
/// more) that represent hours of work. Replacing one would destroy that edit
/// silently and irreversibly.
///
/// `create_new` is used deliberately instead of checking `exists()` first: the
/// check-then-write version has a race, and more importantly it puts the
/// guarantee in a conditional that a later edit could quietly remove. Here the
/// operating system enforces it — if the path exists, the call fails, and there
/// is no code path in which an existing file can be truncated or replaced.
///
/// Returns `Ok(false)` when a file was already there.
fn write_new_file_only(path: &Path, contents: &[u8]) -> Result<bool> {
    use std::io::Write;

    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut f) => {
            f.write_all(contents)?;
            f.sync_all()?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Write `.xmp` sidecars next to the originals, **only where none exists**.
///
/// **This writes to the source drive**, which is why it is never automatic.
/// The original photograph is never opened for writing.
///
/// ## An existing sidecar is never modified
///
/// If a `.xmp` is already beside the photograph it is left exactly as it is and
/// counted in `skipped_existing`. In a working archive that file belongs to
/// Camera Raw, Lightroom or Capture One and contains the develop settings for
/// that image; overwriting it would throw away the edit. There is deliberately
/// no flag to force it — see [`write_new_file_only`].
pub fn write_xmp_sidecars(conn: &Connection, file_ids: &[String]) -> Result<SidecarSummary> {
    let subjects = sidecar_subjects(conn, file_ids)?;
    let mut summary = SidecarSummary::default();

    for subject in subjects {
        if subject.people.is_empty() && subject.keywords.is_empty() {
            summary.skipped_nothing_to_say += 1;
            continue;
        }
        let Some(original) = crate::search::resolve_original(conn, &subject.file_id)? else {
            summary.skipped_offline += 1;
            continue;
        };
        let sidecar: PathBuf = original.with_extension("xmp");
        if write_new_file_only(&sidecar, render_xmp(&subject).as_bytes())? {
            summary.written += 1;
            summary.paths.push(sidecar.display().to_string());
        } else {
            summary.skipped_existing += 1;
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(people: &[&str], keywords: &[&str]) -> SidecarSubject {
        SidecarSubject {
            file_id: "f1".into(),
            people: people.iter().map(|s| s.to_string()).collect(),
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            description: Some("wedding, ceremony".into()),
        }
    }

    #[test]
    fn xmp_carries_people_and_keywords_as_dc_subject() {
        let xmp = render_xmp(&subject(&["Aimee", "Kent"], &["wedding", "suit"]));
        assert!(xmp.contains("<dc:subject>"), "{xmp}");
        for expected in ["Aimee", "Kent", "wedding", "suit"] {
            assert!(xmp.contains(&format!("<rdf:li>{expected}</rdf:li>")), "missing {expected}");
        }
        assert!(xmp.contains("adobe:ns:meta/"));
    }

    #[test]
    fn xmp_escapes_characters_that_would_break_the_document() {
        let xmp = render_xmp(&subject(&["Bob & Sue <the neighbours>"], &[]));
        assert!(xmp.contains("Bob &amp; Sue &lt;the neighbours&gt;"), "{xmp}");
        assert!(!xmp.contains("<the neighbours>"));
    }

    /// A realistic Camera Raw sidecar: what actually sits beside a RAW in a
    /// working archive, holding the develop settings for that photograph.
    const EXISTING_SIDECAR: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:Blacks="-14" crs:Clarity="+22" crs:CameraProfile="Camera Neutral"
    crs:ColorGradeGlobalHue="212" crs:AlreadyApplied="True"/>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    /// The safeguard. An existing sidecar holds someone's edit; it must come
    /// through byte-for-byte identical, and be reported rather than silently
    /// skipped.
    #[test]
    fn an_existing_sidecar_is_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("_DSC2992.xmp");
        std::fs::write(&sidecar, EXISTING_SIDECAR).unwrap();
        let before = std::fs::read(&sidecar).unwrap();
        let before_meta = std::fs::metadata(&sidecar).unwrap();

        let wrote = write_new_file_only(&sidecar, b"keywords only, would destroy the edit").unwrap();

        assert!(!wrote, "must refuse to write over an existing sidecar");
        assert_eq!(
            std::fs::read(&sidecar).unwrap(),
            before,
            "the develop settings must be byte-for-byte unchanged"
        );
        assert_eq!(
            std::fs::metadata(&sidecar).unwrap().len(),
            before_meta.len()
        );
        // And the content that matters is demonstrably still there.
        let after = std::fs::read_to_string(&sidecar).unwrap();
        assert!(after.contains("crs:Clarity=\"+22\""));
        assert!(after.contains("crs:CameraProfile"));
    }

    #[test]
    fn a_sidecar_is_written_only_where_none_exists() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("_DSC3000.xmp");

        assert!(write_new_file_only(&fresh, b"new").unwrap(), "should write");
        assert_eq!(std::fs::read(&fresh).unwrap(), b"new");

        // A second attempt must not replace what is now there.
        assert!(!write_new_file_only(&fresh, b"replacement").unwrap());
        assert_eq!(std::fs::read(&fresh).unwrap(), b"new");
    }

    /// There must be no way to ask for an overwrite. If this ever compiles
    /// against an `overwrite`/`force` parameter, the safeguard has been undone.
    #[test]
    fn the_writer_exposes_no_way_to_force_an_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.xmp");
        std::fs::write(&p, "original").unwrap();
        // The only entry point takes a path and bytes — nothing else.
        let _: fn(&Path, &[u8]) -> Result<bool> = write_new_file_only;
        assert!(!write_new_file_only(&p, b"x").unwrap());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "original");
    }

    #[test]
    fn copying_nothing_is_refused_rather_than_silently_doing_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_in_memory(crate::db::SchemaKind::Archive).unwrap();
        assert!(copy_photos(&conn, &[], dir.path()).is_err());
    }

    #[test]
    fn summary_tells_the_user_which_drive_to_connect() {
        let s = ExportSummary {
            copied: 12,
            skipped_offline: 30,
            drives_to_connect: vec![5, 6],
            destination: "/Users/wayne/Desktop/Aimee".into(),
            ..Default::default()
        };
        let line = s.summary();
        assert!(line.contains("Copied 12 photograph(s)"), "{line}");
        assert!(line.contains("30 more are on Drive 5, 6"), "{line}");
    }
}
