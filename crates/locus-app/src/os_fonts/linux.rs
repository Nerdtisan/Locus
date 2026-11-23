// SPDX-License-Identifier: AGPL-3.0-or-later
//! Linux: fontconfig's list through `fc-list`, when fontconfig is
//! installed (#477). It covers the user's configured font folders, which
//! the folder scan can miss. Without `fc-list` the folder scan is used.
//! Paths stay in memory and are never logged.

use super::{Face, Locator};
use std::path::PathBuf;

/// One line per face: family, style, collection index and file.
const FORMAT: &str = "%{family[0]}\\t%{style[0]}\\t%{index}\\t%{file}\\n";

pub(super) fn faces() -> Vec<Face> {
    let Ok(out) = std::process::Command::new("fc-list")
        .args(["--format", FORMAT, ":outline=true"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// Faces from `fc-list` output; malformed lines and files that aren't
/// TrueType or OpenType are skipped.
pub(super) fn parse(text: &str) -> Vec<Face> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.splitn(4, '\t');
            let (family, style, index, file) = (f.next()?, f.next()?, f.next()?, f.next()?);
            let path = PathBuf::from(file);
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            if !["ttf", "otf", "ttc", "otc"].contains(&ext.as_str()) {
                return None;
            }
            Some(Face {
                family: family.to_owned(),
                style: style.to_owned(),
                at: Locator::File {
                    path,
                    index: Some(index.parse().ok()?),
                },
            })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn fc_list_lines_become_faces() {
        let text = "DejaVu Sans\tBook\t0\t/usr/share/fonts/DejaVuSans.ttf\n\
                    Noto Sans CJK\tBold\t2\t/usr/share/fonts/NotoSansCJK-Bold.ttc\n\
                    Old Bitmap\tRegular\t0\t/usr/share/fonts/old.pcf.gz\n\
                    broken line\n\
                    Bad Index\tRegular\tx\t/f.ttf\n";
        let faces = parse(text);
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].family, "DejaVu Sans");
        assert_eq!(faces[0].style, "Book");
        assert_eq!(
            faces[1].at,
            Locator::File {
                path: PathBuf::from("/usr/share/fonts/NotoSansCJK-Bold.ttc"),
                index: Some(2),
            }
        );
    }
}
