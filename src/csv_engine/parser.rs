use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use crate::csv_engine::types::*;

pub struct IndexResult {
    pub columns: Vec<CsvColumn>,
    pub row_offsets: Vec<u64>,
    pub total_rows: usize,
    pub file_size_bytes: u64,
    pub has_headers: bool,
    pub delimiter: u8,
}

pub fn detect_delimiter(path: &Path) -> u8 {
    use std::io::Read;

    let candidates: &[u8] = &[b',', b';', b'\t', b'|'];

    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return b',',
    };

    let mut buf = vec![0u8; 8192];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return b',',
    };
    buf.truncate(n);

    if buf.is_empty() {
        return b',';
    }

    let mut best_delim = b',';
    let mut best_score: i64 = -1;

    for &delim in candidates {
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .delimiter(delim)
            .flexible(true)
            .from_reader(buf.as_slice());

        let mut counts: Vec<usize> = Vec::new();
        for result in rdr.records() {
            match result {
                Ok(record) => counts.push(record.len()),
                Err(_) => break,
            }
            if counts.len() >= 10 {
                break;
            }
        }

        if counts.is_empty() {
            continue;
        }

        let first = counts[0];
        if first <= 1 {
            continue;
        }
        let consistent = counts.iter().all(|&c| c == first);
        let score = if consistent {
            first as i64 * 100
        } else {
            first as i64
        };

        if score > best_score {
            best_score = score;
            best_delim = delim;
        }
    }

    best_delim
}

#[must_use]
pub fn index_file(path: &Path) -> Result<IndexResult, String> {
    index_file_with_progress(path, |_, _| {})
}

#[must_use]
pub fn index_file_with_progress<F>(path: &Path, on_progress: F) -> Result<IndexResult, String>
where
    F: Fn(u64, u64),
{
    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let file_size_bytes = file
        .metadata()
        .map_err(|e| format!("Failed to read metadata: {}", e))?
        .len();

    let delimiter = detect_delimiter(path);

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(BufReader::new(&file));

    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| format!("Failed to read headers: {}", e))?
        .iter()
        .map(|h| h.to_string())
        .collect();

    let mut row_offsets: Vec<u64> = Vec::new();
    let mut sample_rows: Vec<Vec<String>> = Vec::new();
    let mut total_rows = 0;
    let mut last_progress_byte: u64 = 0;
    let progress_byte_interval = (file_size_bytes / 100).max(1);

    for result in reader.records() {
        let record = result.map_err(|e| format!("Failed to read record: {}", e))?;
        let pos = record.position().ok_or("Missing position")?;
        let byte_offset = pos.byte();
        row_offsets.push(byte_offset);

        if total_rows < 1000 {
            sample_rows.push(record.iter().map(|f| f.to_string()).collect());
        }

        total_rows += 1;

        if byte_offset - last_progress_byte >= progress_byte_interval {
            on_progress(byte_offset, file_size_bytes);
            last_progress_byte = byte_offset;
        }
    }

    on_progress(file_size_bytes, file_size_bytes);

    let columns = infer_columns(&headers, &sample_rows);

    Ok(IndexResult {
        columns,
        row_offsets,
        total_rows,
        file_size_bytes,
        has_headers: true,
        delimiter,
    })
}

fn infer_columns(headers: &[String], sample_rows: &[Vec<String>]) -> Vec<CsvColumn> {
    headers
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let inferred_type = infer_column_type(sample_rows, i);
            CsvColumn {
                index: i,
                name: name.clone(),
                inferred_type,
            }
        })
        .collect()
}

fn infer_column_type(sample_rows: &[Vec<String>], col_index: usize) -> ColumnType {
    let mut is_number = true;
    let mut is_boolean = true;
    let mut has_values = false;

    for row in sample_rows {
        let value = match row.get(col_index) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };
        has_values = true;

        if is_number && value.parse::<f64>().is_err() {
            is_number = false;
        }
        if is_boolean
            && !matches!(
                value.to_lowercase().as_str(),
                "true" | "false" | "0" | "1" | "yes" | "no"
            )
        {
            is_boolean = false;
        }

        if !is_number && !is_boolean {
            break;
        }
    }

    if !has_values {
        return ColumnType::String;
    }
    if is_number {
        ColumnType::Number
    } else if is_boolean {
        ColumnType::Boolean
    } else {
        ColumnType::String
    }
}

pub fn read_single_row_from_reader(
    reader: &mut BufReader<File>,
    row_offsets: &[u64],
    row_index: usize,
    col_count: usize,
    delimiter: u8,
) -> Result<Vec<String>, String> {
    if row_index >= row_offsets.len() {
        return Ok(vec![String::new(); col_count]);
    }

    reader
        .seek(SeekFrom::Start(row_offsets[row_index]))
        .map_err(|e| format!("Failed to seek: {}", e))?;

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("Failed to read line: {}", e))?;

    let mut csv_reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(line.as_bytes());

    match csv_reader.records().next() {
        Some(Ok(record)) => {
            let mut row: Vec<String> = record.iter().map(|s| s.to_string()).collect();
            while row.len() < col_count {
                row.push(String::new());
            }
            Ok(row)
        }
        Some(Err(e)) => Err(format!("Failed to read record: {}", e)),
        None => Ok(vec![String::new(); col_count]),
    }
}

pub fn read_single_row_with_delim(
    path: &Path,
    row_offsets: &[u64],
    row_index: usize,
    col_count: usize,
    delimiter: u8,
) -> Result<Vec<String>, String> {
    if row_index >= row_offsets.len() {
        return Ok(vec![String::new(); col_count]);
    }
    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let mut reader = BufReader::new(file);
    read_single_row_from_reader(&mut reader, row_offsets, row_index, col_count, delimiter)
}

pub fn read_chunk_with_delim(
    path: &Path,
    row_offsets: &[u64],
    edits: &HashMap<(usize, usize), String>,
    start: usize,
    count: usize,
    col_count: usize,
    delimiter: u8,
) -> Result<RowChunk, String> {
    if start >= row_offsets.len() {
        return Ok(RowChunk {
            start_index: start,
            rows: vec![],
        });
    }

    let end = (start + count).min(row_offsets.len());
    let byte_offset = row_offsets[start];

    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let mut buf_reader = BufReader::new(file);
    buf_reader
        .seek(SeekFrom::Start(byte_offset))
        .map_err(|e| format!("Failed to seek: {}", e))?;

    let mut rows: Vec<Vec<String>> = Vec::with_capacity(end - start);
    let mut csv_reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(buf_reader);

    for (i, result) in csv_reader.records().enumerate() {
        if i >= (end - start) {
            break;
        }
        let record = result.map_err(|e| format!("Failed to read record: {}", e))?;
        let row_index = start + i;

        let mut row: Vec<String> = Vec::with_capacity(col_count);
        for col_index in 0..col_count {
            let value = if let Some(edited) = edits.get(&(row_index, col_index)) {
                edited.clone()
            } else {
                record.get(col_index).unwrap_or("").to_string()
            };
            row.push(value);
        }
        rows.push(row);
    }

    Ok(RowChunk {
        start_index: start,
        rows,
    })
}

pub fn search_rows(
    path: &Path,
    row_offsets: &[u64],
    edits: &HashMap<(usize, usize), String>,
    query: &str,
    column_index: Option<usize>,
    col_count: usize,
    delimiter: u8,
) -> Result<SearchResult, String> {
    crate::csv_engine::query::search_rows(
        path,
        row_offsets,
        edits,
        query,
        column_index,
        col_count,
        delimiter,
    )
}

pub fn filter_rows(
    path: &Path,
    row_offsets: &[u64],
    edits: &HashMap<(usize, usize), String>,
    criteria: &[FilterCriteria],
    _col_count: usize,
    delimiter: u8,
) -> Result<Vec<usize>, String> {
    crate::csv_engine::query::filter_rows(path, row_offsets, edits, criteria, delimiter)
}

pub fn sort_rows(
    path: &Path,
    row_offsets: &[u64],
    edits: &HashMap<(usize, usize), String>,
    column_index: usize,
    ascending: bool,
    delimiter: u8,
) -> Result<Vec<usize>, String> {
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

    let mut values: Vec<(usize, String)> = Vec::with_capacity(row_offsets.len());

    for (row_idx, result) in csv_reader.records().enumerate() {
        if row_idx >= row_offsets.len() {
            break;
        }
        let record = result.map_err(|e| format!("Failed to read record: {}", e))?;
        let value = if let Some(edited) = edits.get(&(row_idx, column_index)) {
            edited.clone()
        } else {
            record.get(column_index).unwrap_or("").to_string()
        };
        values.push((row_idx, value));
    }

    values.sort_by(|a, b| {
        let cmp = match (a.1.parse::<f64>(), b.1.parse::<f64>()) {
            (Ok(na), Ok(nb)) => na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal),
            _ => a.1.to_lowercase().cmp(&b.1.to_lowercase()),
        };
        if ascending {
            cmp
        } else {
            cmp.reverse()
        }
    });

    Ok(values.into_iter().map(|(idx, _)| idx).collect())
}

/// Aggregate statistics for a column — streams sequentially (fast for large files)
pub fn aggregate_column(
    path: &Path,
    column_index: usize,
    row_offsets: &[u64],
    edits: &HashMap<(usize, usize), String>,
    delimiter: u8,
) -> Result<ColumnStats, String> {
    let file =
        File::open(path).map_err(|e| format!("Failed to open file for aggregation: {}", e))?;
    let mut buf_reader = BufReader::new(file);

    // Seek to start of data (first row offset) and stream sequentially
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

    let mut sum: f64 = 0.0;
    let mut min_num: Option<f64> = None;
    let mut max_num: Option<f64> = None;
    let mut min_length: Option<usize> = None;
    let mut max_length: Option<usize> = None;
    let mut count: usize = 0;
    let mut numeric_count: usize = 0;

    for (row_idx, result) in csv_reader.records().enumerate() {
        if row_idx >= row_offsets.len() {
            break;
        }
        let record = result.map_err(|e| format!("Failed to read record: {}", e))?;

        let value = if let Some(edited) = edits.get(&(row_idx, column_index)) {
            edited.as_str()
        } else {
            record.get(column_index).unwrap_or("")
        };

        count += 1;
        let len = value.len();
        min_length = Some(min_length.map_or(len, |m: usize| m.min(len)));
        max_length = Some(max_length.map_or(len, |m: usize| m.max(len)));

        let trimmed = value.trim();
        if !trimmed.is_empty() {
            if let Ok(num) = trimmed.parse::<f64>() {
                if num.is_finite() {
                    numeric_count += 1;
                    sum += num;
                    min_num = Some(min_num.map_or(num, |m: f64| m.min(num)));
                    max_num = Some(max_num.map_or(num, |m: f64| m.max(num)));
                }
            }
        }
    }

    let (final_sum, final_avg) = if numeric_count > 0 {
        (Some(sum), Some(sum / numeric_count as f64))
    } else {
        (None, None)
    };

    Ok(ColumnStats {
        sum: final_sum,
        avg: final_avg,
        min: min_num,
        max: max_num,
        count,
        numeric_count,
        min_length,
        max_length,
    })
}
