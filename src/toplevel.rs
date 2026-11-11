//! Window selection and `--list` output. Pure functions, unit tested; the
//! Wayland side is SCTK's `ForeignToplevelList`.

/// The state of one ext foreign toplevel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Toplevel {
    pub identifier: String,
    pub app_id: String,
    pub title: String,
}

/// Pick the toplevels to show, as indices into `toplevels`. With no `ids`
/// that's every toplevel in announcement order; otherwise the toplevels whose
/// identifier exactly matches one of `ids`, in the order of `ids`. Unknown IDs
/// are passed to `unknown` and skipped; duplicates are shown once. Empty
/// identifiers never match.
pub fn select(toplevels: &[Toplevel], ids: &[String], mut unknown: impl FnMut(&str)) -> Vec<usize> {
    if ids.is_empty() {
        return (0..toplevels.len()).collect();
    }
    let mut out = Vec::new();
    for id in ids {
        match toplevels
            .iter()
            .position(|t| !t.identifier.is_empty() && t.identifier == *id)
        {
            Some(i) if !out.contains(&i) => out.push(i),
            Some(_) => {}
            None => unknown(id),
        }
    }
    out
}

/// `identifier<TAB>app_id<TAB>title` per toplevel, one per line.
pub fn format_list(toplevels: &[Toplevel]) -> String {
    // Tabs and newlines would break the line/column format.
    let field = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    toplevels
        .iter()
        .map(|t| {
            format!(
                "{}\t{}\t{}\n",
                field(&t.identifier),
                field(&t.app_id),
                field(&t.title)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toplevels() -> Vec<Toplevel> {
        ["10", "2", "abc", "100", "x y"]
            .into_iter()
            .map(|id| Toplevel {
                identifier: id.into(),
                app_id: "app".into(),
                title: "title".into(),
            })
            .collect()
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn select_quiet(tls: &[Toplevel], wanted: &[&str]) -> Vec<usize> {
        select(tls, &ids(wanted), |_| {})
    }

    #[test]
    fn all_windows_in_announcement_order() {
        assert_eq!(select_quiet(&toplevels(), &[]), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn exact_match_in_id_order() {
        assert_eq!(select_quiet(&toplevels(), &["100", "10", "1", "x"]), [3, 0]);
    }

    #[test]
    fn duplicates_shown_once() {
        // Spaces are part of the ID.
        assert_eq!(select_quiet(&toplevels(), &["2", "x y", "2"]), [1, 4]);
    }

    #[test]
    fn unknown_ids_reported() {
        let mut unknown = Vec::new();
        let picked = select(&toplevels(), &ids(&["nope", "abc"]), |id| {
            unknown.push(id.to_string())
        });
        assert_eq!(picked, [2]);
        assert_eq!(unknown, ["nope"]);
        assert!(select_quiet(&toplevels(), &["zzz"]).is_empty());
    }

    #[test]
    fn empty_identifier_never_matches() {
        let mut tls = toplevels();
        tls[1].identifier.clear();
        assert_eq!(select_quiet(&tls, &["2", "", "abc"]), [2]);
    }

    #[test]
    fn list_format() {
        let mut tls = toplevels();
        tls[2].title = "a\tb\nc".into();
        let out = format_list(&tls);
        assert!(out.contains("abc\tapp\ta b c\n"), "{out:?}");
        assert_eq!(out.lines().count(), 5);
    }
}
