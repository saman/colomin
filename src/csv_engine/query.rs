#![allow(dead_code)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::Path;

use regex::RegexBuilder;

use crate::csv_engine::types::{FilterCriteria, FilterOp, SearchMatch, SearchResult};

const MAX_SEARCH_MATCHES: usize = 20_000;

enum SearchMatcher {
    Plain {
        query: String,
        case_sensitive: bool,
    },
    Regex(regex::Regex),
}

impl SearchMatcher {
    fn new(query: &str, case_sensitive: bool, regex_enabled: bool) -> Option<Self> {
        if regex_enabled {
            RegexBuilder::new(query)
                .case_insensitive(!case_sensitive)
                .build()
                .ok()
                .map(Self::Regex)
        } else {
            Some(Self::Plain {
                query: if case_sensitive { query.to_string() } else { query.to_lowercase() },
                case_sensitive,
            })
        }
    }

    fn is_match(&self, value: &str) -> bool {
        match self {
            Self::Plain { query, case_sensitive } => {
                if *case_sensitive {
                    value.contains(query)
                } else {
                    value.to_lowercase().contains(query)
                }
            }
            Self::Regex(re) => re.is_match(value),
        }
    }
}

pub fn search_rows(
    path: &Path,
    row_offsets: &[u64],
    row_sources: Option<&[(usize, usize)]>,
    edits: &HashMap<(usize, usize), String>,
    query: &str,
    column_index: Option<usize>,
    col_count: usize,
    delimiter: u8,
    case_sensitive: bool,
    regex_enabled: bool,
) -> Result<SearchResult, String> {
    let Some(matcher) = SearchMatcher::new(query, case_sensitive, regex_enabled) else {
        return Ok(SearchResult {
            matches: Vec::new(),
            row_indices: Vec::new(),
            total_matches: 0,
        });
    };
    let mut matching_indices: Vec<usize> = Vec::new();
    let mut matching_cells: Vec<SearchMatch> = Vec::new();

    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let mut buf_reader = BufReader::new(file);

    if let Some(row_sources) = row_sources {
        for &(display_row, source_row) in row_sources {
            let Some(offset) = row_offsets.get(source_row).copied() else { continue };
            buf_reader
                .seek(SeekFrom::Start(offset))
                .map_err(|e| format!("Failed to seek: {}", e))?;
            let mut csv_reader = csv::ReaderBuilder::new()
                .has_headers(false)
                .flexible(true)
                .delimiter(delimiter)
                .from_reader(&mut buf_reader);
            let Some(result) = csv_reader.records().next() else { continue };
            let record = result.map_err(|e| format!("Failed to read record: {}", e))?;

            if !collect_record_matches(
                &matcher,
                &record,
                edits,
                display_row,
                column_index,
                col_count,
                &mut matching_indices,
                &mut matching_cells,
            ) {
                break;
            }
        }

        let total_matches = matching_cells.len();
        return Ok(SearchResult {
            matches: matching_cells,
            row_indices: matching_indices,
            total_matches,
        });
    }

    if !row_offsets.is_empty() {
        buf_reader
            .seek(SeekFrom::Start(row_offsets[0]))
            .map_err(|e| format!("Failed to seek: {}", e))?;
    }

    let mut csv_reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(buf_reader);

    for (row_idx, result) in csv_reader.records().enumerate() {
        if row_idx >= row_offsets.len() {
            break;
        }
        let record = result.map_err(|e| format!("Failed to read record: {}", e))?;

        if !collect_record_matches(
            &matcher,
            &record,
            edits,
            row_idx,
            column_index,
            col_count,
            &mut matching_indices,
            &mut matching_cells,
        ) {
            break;
        }
    }

    let total_matches = matching_cells.len();
    Ok(SearchResult {
        matches: matching_cells,
        row_indices: matching_indices,
        total_matches,
    })
}

fn collect_record_matches(
    matcher: &SearchMatcher,
    record: &csv::StringRecord,
    edits: &HashMap<(usize, usize), String>,
    result_row_index: usize,
    column_index: Option<usize>,
    col_count: usize,
    matching_indices: &mut Vec<usize>,
    matching_cells: &mut Vec<SearchMatch>,
) -> bool {
    let mut row_matched = false;
    let mut limit_reached = false;

    if let Some(col_idx) = column_index {
        let value = if let Some(edited) = edits.get(&(result_row_index, col_idx)) {
            edited.as_str()
        } else {
            record.get(col_idx).unwrap_or("")
        };
        if matcher.is_match(value) {
            row_matched = true;
            limit_reached = !push_search_match(matching_cells, result_row_index, col_idx);
        }
    } else {
        for col_idx in 0..col_count {
            let value = if let Some(edited) = edits.get(&(result_row_index, col_idx)) {
                edited.as_str()
            } else {
                record.get(col_idx).unwrap_or("")
            };

            if matcher.is_match(value) {
                row_matched = true;
                if !push_search_match(matching_cells, result_row_index, col_idx) {
                    limit_reached = true;
                    break;
                }
            }
        }
    }

    if row_matched {
        matching_indices.push(result_row_index);
    }

    !limit_reached
}

fn push_search_match(
    matching_cells: &mut Vec<SearchMatch>,
    row_index: usize,
    column_index: usize,
) -> bool {
    if matching_cells.len() >= MAX_SEARCH_MATCHES {
        return false;
    }
    matching_cells.push(SearchMatch {
        row_index,
        column_index,
    });
    matching_cells.len() < MAX_SEARCH_MATCHES
}

pub fn filter_rows(
    path: &Path,
    row_offsets: &[u64],
    edits: &HashMap<(usize, usize), String>,
    criteria: &[FilterCriteria],
    delimiter: u8,
) -> Result<Vec<usize>, String> {
    if criteria.is_empty() {
        return Ok((0..row_offsets.len()).collect());
    }

    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let mut buf_reader = BufReader::new(file);

    if !row_offsets.is_empty() {
        buf_reader
            .seek(SeekFrom::Start(row_offsets[0]))
            .map_err(|e| format!("Failed to seek: {}", e))?;
    }

    let mut csv_reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(buf_reader);

    let mut matching: Vec<usize> = Vec::new();

    for (row_idx, result) in csv_reader.records().enumerate() {
        if row_idx >= row_offsets.len() {
            break;
        }
        let record = result.map_err(|e| format!("Failed to read record: {}", e))?;

        let all_match = criteria.iter().all(|c| {
            let value = if let Some(edited) = edits.get(&(row_idx, c.column_index)) {
                edited.as_str()
            } else {
                record.get(c.column_index).unwrap_or("")
            };
            matches_filter(value, c)
        });

        if all_match {
            matching.push(row_idx);
        }
    }

    Ok(matching)
}

fn matches_filter(value: &str, criteria: &FilterCriteria) -> bool {
    let val_lower = value.to_lowercase();
    let crit_lower = criteria.value.to_lowercase();

    match criteria.operator {
        FilterOp::Contains => val_lower.contains(&crit_lower),
        FilterOp::Equals => val_lower == crit_lower,
        FilterOp::StartsWith => val_lower.starts_with(&crit_lower),
        FilterOp::GreaterThan => match (value.parse::<f64>(), criteria.value.parse::<f64>()) {
            (Ok(a), Ok(b)) => a > b,
            _ => val_lower > crit_lower,
        },
        FilterOp::LessThan => match (value.parse::<f64>(), criteria.value.parse::<f64>()) {
            (Ok(a), Ok(b)) => a < b,
            _ => val_lower < crit_lower,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempCsv {
        path: PathBuf,
    }

    impl TempCsv {
        fn write(contents: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "colomin-search-test-{}-{}.csv",
                std::process::id(),
                nanos
            ));
            std::fs::write(&path, contents).expect("test CSV should be writable");
            Self { path }
        }
    }

    impl Drop for TempCsv {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn row_offsets(contents: &str) -> Vec<u64> {
        let mut offsets = vec![0];
        for (idx, byte) in contents.bytes().enumerate() {
            if byte == b'\n' && idx + 1 < contents.len() {
                offsets.push((idx + 1) as u64);
            }
        }
        offsets
    }

    #[test]
    fn search_rows_uses_display_row_for_mapped_edits() {
        let contents = "source0,old\nsource1,old\n";
        let csv = TempCsv::write(contents);
        let offsets = row_offsets(contents);
        let row_sources = vec![(0, 1), (1, 0)];
        let edits = HashMap::from([((0, 0), "needle".to_string())]);

        let result = search_rows(
            &csv.path,
            &offsets,
            Some(&row_sources),
            &edits,
            "needle",
            None,
            2,
            b',',
            false,
            false,
        )
        .expect("search should succeed");

        assert_eq!(result.matches, vec![SearchMatch { row_index: 0, column_index: 0 }]);
    }

    #[test]
    fn search_rows_caps_collected_cell_matches() {
        let mut contents = String::new();
        for _ in 0..(MAX_SEARCH_MATCHES + 5) {
            contents.push_str("needle\n");
        }
        let csv = TempCsv::write(&contents);
        let offsets = row_offsets(&contents);
        let edits = HashMap::new();

        let result = search_rows(
            &csv.path,
            &offsets,
            None,
            &edits,
            "needle",
            None,
            1,
            b',',
            false,
            false,
        )
        .expect("search should succeed");

        assert_eq!(result.matches.len(), MAX_SEARCH_MATCHES);
        assert_eq!(result.row_indices.len(), MAX_SEARCH_MATCHES);
        assert_eq!(result.total_matches, MAX_SEARCH_MATCHES);
    }
}
