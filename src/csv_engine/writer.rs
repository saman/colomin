use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::Path;

use crate::csv_engine::parser;
use crate::state::{ColSource, OpenFile, RowSource};

pub fn save_file(
    open_file: &OpenFile,
    target_path: &Path,
    _headers: &[String],
) -> Result<(), String> {
    let temp_path = target_path.with_extension("csv.tmp");
    let out_file =
        File::create(&temp_path).map_err(|e| format!("Failed to create temp file: {}", e))?;
    let delimiter = open_file.delimiter;
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(BufWriter::new(out_file));

    let has_structural = open_file.row_order.is_some() || open_file.col_order.is_some();

    let headers: Vec<String> = if let Some(ref col_order) = open_file.col_order {
        col_order
            .iter()
            .enumerate()
            .map(|(_new_idx, src)| match src {
                ColSource::Original(orig_idx) => open_file
                    .metadata
                    .columns
                    .iter()
                    .find(|c| c.index == *orig_idx)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| format!("Column {}", orig_idx + 1)),
                ColSource::Inserted(ins_idx) => open_file
                    .inserted_columns
                    .get(*ins_idx)
                    .cloned()
                    .unwrap_or_else(|| "New Column".to_string()),
            })
            .collect()
    } else {
        open_file
            .metadata
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect()
    };

    writer
        .write_record(&headers)
        .map_err(|e| format!("Failed to write headers: {}", e))?;

    if !has_structural {
        let file = File::open(&open_file.file_path)
            .map_err(|e| format!("Failed to open source: {}", e))?;
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .delimiter(delimiter)
            .from_reader(BufReader::new(file));

        for (row_idx, result) in reader.records().enumerate() {
            if row_idx >= open_file.row_offsets.len() {
                break;
            }
            let record = result.map_err(|e| format!("Failed to read record: {}", e))?;

            let row: Vec<String> = (0..headers.len())
                .map(|col_idx| {
                    if let Some(edited) = open_file.edits.get(&(row_idx, col_idx)) {
                        edited.clone()
                    } else {
                        record.get(col_idx).unwrap_or("").to_string()
                    }
                })
                .collect();

            writer
                .write_record(&row)
                .map_err(|e| format!("Failed to write record: {}", e))?;
        }
    } else {
        let effective_rows = open_file.effective_row_count();
        let col_count = open_file.current_col_count();

        for virtual_row in 0..effective_rows {
            let row_src = open_file.resolve_row(virtual_row);
            let mut row: Vec<String> = Vec::with_capacity(col_count);

            for virtual_col in 0..col_count {
                let col_src = open_file.resolve_col(virtual_col);

                let value = match (&row_src, &col_src) {
                    (RowSource::Original(orig_row), ColSource::Original(orig_col)) => {
                        if let Some(edited) = open_file.edits.get(&(*orig_row, *orig_col)) {
                            edited.clone()
                        } else {
                            let single = parser::read_single_row_with_delim(
                                &open_file.file_path,
                                &open_file.row_offsets,
                                *orig_row,
                                open_file.original_col_count,
                                delimiter,
                            )
                            .unwrap_or_else(|_| vec![String::new(); open_file.original_col_count]);
                            single.get(*orig_col).cloned().unwrap_or_default()
                        }
                    }
                    (RowSource::Inserted(ins_row), ColSource::Original(_)) => open_file
                        .inserted_rows
                        .get(*ins_row)
                        .and_then(|r| r.get(virtual_col))
                        .cloned()
                        .unwrap_or_default(),
                    (RowSource::Original(_), ColSource::Inserted(ins_col)) => open_file
                        .inserted_col_values
                        .get(&(virtual_row, *ins_col))
                        .cloned()
                        .unwrap_or_default(),
                    (RowSource::Inserted(ins_row), ColSource::Inserted(ins_col)) => open_file
                        .inserted_col_values
                        .get(&(*ins_row, *ins_col))
                        .cloned()
                        .unwrap_or_default(),
                };

                row.push(value);
            }

            writer
                .write_record(&row)
                .map_err(|e| format!("Failed to write record: {}", e))?;
        }
    }

    writer
        .flush()
        .map_err(|e| format!("Failed to flush writer: {}", e))?;

    fs::rename(&temp_path, target_path)
        .map_err(|e| format!("Failed to rename temp file: {}", e))?;

    Ok(())
}
