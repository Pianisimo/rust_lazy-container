//! Parsing for the space-padded tables the `container` CLI prints.
//!
//! Rows are sliced by the header's byte offsets rather than split on
//! whitespace: cells are legitimately empty (a stopped container has no
//! address) and values can contain spaces ("1024 MB").

/// Header names paired with the byte offset each one starts at.
///
/// Columns are separated by two or more spaces, so a single space stays part
/// of the name — `INDEX DIGEST` and `FULL SIZE` are one column each.
pub(crate) fn columns(header: &str) -> Vec<(&str, usize)> {
    let mut columns = Vec::new();
    let mut index = 0;

    while index < header.len() {
        while header[index..].starts_with(' ') {
            index += 1;
        }
        if index >= header.len() {
            break;
        }

        let start = index;
        let mut end = index;
        loop {
            match header[end..].find(' ') {
                Some(offset) => {
                    let space = end + offset;
                    if header[space..].starts_with("  ") {
                        end = space;
                        break;
                    }
                    end = space + 1;
                }
                None => {
                    end = header.len();
                    break;
                }
            }
        }

        columns.push((header[start..end].trim_end(), start));
        index = end;
    }

    columns
}

/// Byte range of the first column matching `names`, running up to the next
/// column (or the end of the row for the last one).
pub(crate) fn span(columns: &[(&str, usize)], names: &[&str]) -> Option<(usize, usize)> {
    let index = columns.iter().position(|(name, _)| names.contains(name))?;
    let end = columns
        .get(index + 1)
        .map(|(_, start)| *start)
        .unwrap_or(usize::MAX);
    Some((columns[index].1, end))
}

/// The trimmed cell at `span`, or an empty string if the row stops short.
pub(crate) fn cell(line: &str, (start, end): (usize, usize)) -> String {
    line.get(start.min(line.len())..end.min(line.len()))
        .unwrap_or("")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_word_headers_split_on_padding() {
        let header = "ID        IMAGE   STATE    IP";
        let names: Vec<&str> = columns(header).iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["ID", "IMAGE", "STATE", "IP"]);
    }

    #[test]
    fn a_single_space_stays_inside_one_column_name() {
        // `container image ls --verbose` has two-word headers.
        let header = "NAME     TAG     INDEX DIGEST  OS     FULL SIZE  MANIFEST DIGEST";
        let names: Vec<&str> = columns(header).iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "NAME",
                "TAG",
                "INDEX DIGEST",
                "OS",
                "FULL SIZE",
                "MANIFEST DIGEST"
            ]
        );
    }

    #[test]
    fn spans_cover_the_whole_two_word_column() {
        let header = "NAME     TAG     INDEX DIGEST  OS";
        let row = "alpine   edge    020dfcbaaf4c  linux";
        let columns = columns(header);

        let digest = span(&columns, &["INDEX DIGEST", "DIGEST"]).unwrap();
        assert_eq!(cell(row, digest), "020dfcbaaf4c");
        assert_eq!(cell(row, span(&columns, &["OS"]).unwrap()), "linux");
    }

    #[test]
    fn a_row_that_stops_early_yields_empty_cells() {
        let header = "ID     STATE    IP";
        let row = "alpha  stopped";
        let columns = columns(header);
        assert_eq!(cell(row, span(&columns, &["IP"]).unwrap()), "");
    }
}
