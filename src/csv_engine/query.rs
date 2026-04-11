#![allow(dead_code)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::Path;

use crate::csv_engine::types::{FilterCriteria, FilterOp, SearchResult};

pub fn search_rows(
    path: &Path,
    row_offsets: &[u64],
    edits: &HashMap<(usize, usize), String>,
    query: &str,
    column_index: Option<usize>,
    col_count: usize,
    delimiter: u8,
) -> Result<SearchResult, String> {
    let query_lower = query.to_lowercase();
    let mut matching_indices: Vec<usize> = Vec::new();

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

    for (row_idx, result) in csv_reader.records().enumerate() {
        if row_idx >= row_offsets.len() {
            break;
        }
        let record = result.map_err(|e| format!("Failed to read record: {}", e))?;

        let mut found = false;
        if let Some(col_idx) = column_index {
            let value = if let Some(edited) = edits.get(&(row_idx, col_idx)) {
                edited.as_str()
            } else {
                record.get(col_idx).unwrap_or("")
            };
            found = value.to_lowercase().contains(&query_lower);
        } else {
            for col_idx in 0..col_count {
                let value = if let Some(edited) = edits.get(&(row_idx, col_idx)) {
                    edited.as_str()
                } else {
                    record.get(col_idx).unwrap_or("")
                };

                if value.to_lowercase().contains(&query_lower) {
                    found = true;
                    break;
                }
            }
        }

        if found {
            matching_indices.push(row_idx);
        }
    }

    let total_matches = matching_indices.len();
    Ok(SearchResult {
        row_indices: matching_indices,
        total_matches,
    })
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
