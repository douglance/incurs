//! Text shown to a person rather than sent to a command.

/// Turns an identifier into a label a person reads.
///
/// Property names, command names, and result keys all arrive as identifiers.
/// This was three byte-identical copies — one per module that needed a label —
/// which is exactly how two of them drift.
pub fn humanize(name: &str) -> String {
    let spaced = name.replace(['_', '-'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identifier_reads_as_words() {
        assert_eq!(humanize("output_dir"), "Output dir");
        assert_eq!(humanize("dry-run"), "Dry run");
        assert_eq!(humanize("title"), "Title");
    }

    #[test]
    fn an_empty_name_stays_empty() {
        assert_eq!(humanize(""), "");
    }
}
