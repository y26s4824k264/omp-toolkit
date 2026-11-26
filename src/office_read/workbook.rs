use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use calamine::{Data, Reader, Xlsx, XlsxFormulaMetadata, expand_shared_formula};
use quick_xml::events::Event;
use serde::Serialize;
use serde_json::{Value, json};

use super::{Page, Selection};

type Position = (u32, u32);

struct Bounds {
    first: Position,
    last: Position,
}

impl Bounds {
    fn parse(input: &str) -> Result<Self> {
        let (first, last) = input.split_once(':').unwrap_or((input, input));
        let first = position(first)?;
        let last = position(last)?;
        ensure!(
            first.0 <= last.0 && first.1 <= last.1,
            "range must run from top left to bottom right"
        );
        let size = u64::from(last.0 - first.0 + 1) * u64::from(last.1 - first.1 + 1);
        ensure!(size <= 4_000, "select at most 4000 cells per range");
        Ok(Self { first, last })
    }

    fn contains(&self, (row, column): Position) -> bool {
        (self.first.0..=self.last.0).contains(&row)
            && (self.first.1..=self.last.1).contains(&column)
    }
}

fn position(input: &str) -> Result<Position> {
    let input = input.trim().strip_prefix('$').unwrap_or(input.trim());
    let split = input
        .find(|c: char| !c.is_ascii_alphabetic())
        .context("missing Excel row")?;
    let (column, row) = input.split_at(split);
    ensure!((1..=3).contains(&column.len()), "invalid Excel column");
    let row = row.strip_prefix('$').unwrap_or(row);
    ensure!(
        row.bytes().all(|byte| byte.is_ascii_digit()),
        "invalid Excel row"
    );
    let row = row.parse::<u32>().context("invalid Excel row")?;
    let column = column.bytes().fold(0_u32, |value, letter| {
        value * 26 + u32::from(letter.to_ascii_uppercase() - b'A' + 1)
    });
    ensure!(
        (1..=1_048_576).contains(&row) && (1..=16_384).contains(&column),
        "cell is outside Excel's grid"
    );
    Ok((row - 1, column - 1))
}

fn address((row, column): Position) -> String {
    let mut column = column + 1;
    let mut letters = Vec::with_capacity(3);
    while column > 0 {
        column -= 1;
        letters.push(char::from(b'A' + (column % 26) as u8));
        column /= 26;
    }
    let column: String = letters.into_iter().rev().collect();
    format!("{column}{}", row + 1)
}

#[derive(Serialize)]
struct Cell {
    address: String,
    value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    formula: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value_type: Option<&'static str>,
}

impl Cell {
    fn push(
        page: &mut Page,
        position: Position,
        data: Data,
        formula: Option<String>,
    ) -> Result<bool> {
        let (value, value_type) = match data {
            Data::Empty if formula.is_none() => return Ok(true),
            Data::Empty => (Value::Null, None),
            Data::Int(value) => (json!(value), None),
            Data::Float(value) => (json!(value), None),
            Data::String(value) => (json!(value), None),
            Data::Bool(value) => (json!(value), None),
            Data::Error(value) => (json!(value.to_string()), Some("error")),
            Data::DateTime(value) => (json!(value.as_f64()), Some("excel_serial")),
            Data::DateTimeIso(value) => (json!(value), Some("iso_datetime")),
            Data::DurationIso(value) => (json!(value), Some("iso_duration")),
        };
        let formula = formula.map(|text| {
            if text.starts_with('=') {
                text
            } else {
                format!("={text}")
            }
        });
        page.push(Self {
            address: address(position),
            value,
            formula,
            value_type,
        })
    }
}

fn read_xlsx(
    workbook: &mut Xlsx<std::io::BufReader<std::fs::File>>,
    sheet: &str,
    bounds: &Bounds,
    page: &mut Page,
) -> Result<()> {
    let mut reader = workbook.worksheet_cells_reader(sheet)?;
    let mut shared = HashMap::new();
    while let Some(cell) = reader.next_cell_with_formula_metadata()? {
        if cell.pos.0 > bounds.last.0 {
            break;
        }
        let selected = bounds.contains(cell.pos);
        let formula = match cell.formula {
            Some(XlsxFormulaMetadata::Shared {
                shared_index,
                formula,
                ..
            }) => {
                shared.insert(shared_index, (cell.pos, formula.clone()));
                Some(formula)
            }
            Some(XlsxFormulaMetadata::SharedDerived { shared_index }) if selected => {
                let (anchor, template) = shared.get(&shared_index).with_context(|| {
                    format!("missing shared formula anchor for {}", address(cell.pos))
                })?;
                Some(expand_shared_formula(template, *anchor, cell.pos)?)
            }
            Some(XlsxFormulaMetadata::Normal { formula }) => Some(formula),
            _ => None,
        };
        if selected && !Cell::push(page, cell.pos, cell.value.into(), formula)? {
            break;
        }
    }
    Ok(())
}

fn select(
    mut workbook: Xlsx<BufReader<File>>,
    selection: &Selection,
    extension: &str,
    page: &mut Page,
) -> Result<Value> {
    let sheets = workbook.sheet_names();
    let Some(sheet) = selection.sheet.as_deref() else {
        ensure!(
            selection.range.is_none(),
            "provide sheet with range, or omit both to list sheets"
        );
        let count = sheets.len();
        for sheet in sheets {
            if !page.push(json!({"name": sheet}))? {
                break;
            }
        }
        return Ok(json!({"format": extension, "unit": "sheets", "count": count}));
    };
    ensure!(
        sheets.iter().any(|name| name == sheet),
        "sheet not found: {sheet}"
    );
    let bounds = Bounds::parse(selection.range.as_deref().unwrap_or("A1:J20"))?;
    if extension == "xls" && (bounds.last.0 >= 65_536 || bounds.last.1 >= 256) {
        bail!("cell is outside the XLS grid");
    }
    read_xlsx(&mut workbook, sheet, &bounds, page)?;
    let range = format!("{}:{}", address(bounds.first), address(bounds.last));
    Ok(json!({"format": extension, "unit": "cells", "sheet": sheet, "range": range}))
}

fn validate_package(file: &mut File) -> Result<()> {
    const MIB: u64 = 1024 * 1024;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut total = 0_u64;
    for index in 0..zip.len() {
        let member = zip.by_index_raw(index)?;
        let size = member.size();
        total = total.saturating_add(size);
        ensure!(
            size <= 128 * MIB && total <= 256 * MIB,
            "XLSX exceeds the expanded ZIP size budget"
        );
        let shared_strings = member
            .name()?
            .rsplit('/')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("sharedStrings.xml"));
        drop(member);
        if shared_strings {
            ensure!(size <= 32 * MIB, "XLSX shared strings exceed 32 MiB");
            let member = zip.by_index(index)?;
            let mut reader =
                quick_xml::Reader::from_reader(BufReader::new(member.take(32 * MIB + 1)));
            let mut buf = Vec::new();
            loop {
                match reader.read_event_into(&mut buf)? {
                    Event::Start(root) | Event::Empty(root)
                        if root.local_name().as_ref() == "sst" =>
                    {
                        if let Some(count) = root.try_get_attribute("uniqueCount")? {
                            let count = count
                                .value
                                .parse::<usize>()
                                .context("invalid XLSX shared string count")?;
                            ensure!(
                                count <= 1_000_000,
                                "XLSX shared string count exceeds 1000000"
                            );
                        }
                        break;
                    }
                    Event::Eof => bail!("XLSX shared strings part has no sst element"),
                    _ => buf.clear(),
                }
            }
        }
    }
    Ok(())
}

pub(super) fn read(
    path: &Path,
    extension: &str,
    selection: &Selection,
    page: &mut Page,
) -> Result<Value> {
    let mut file = File::open(path)?;
    validate_package(&mut file).context("validate XLSX package")?;
    let workbook = Xlsx::new(BufReader::new(file)).context("open XLSX workbook")?;
    select(workbook, selection, extension, page)
}
