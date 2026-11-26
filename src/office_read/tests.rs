use std::{io::Write, path::Path};

use serde_json::{Value, json};
use zip::{ZipWriter, write::SimpleFileOptions};

use super::{Selection, read};

fn write_zip(path: &Path, parts: &[(&str, &str)]) {
    let mut zip = ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, xml) in parts {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

const ROOT_RELS_DOCX: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="main" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="/word/document.xml"/>
</Relationships>"#;

fn open(path: &Path, selection: Selection, format: &str) -> Value {
    serde_json::from_str(&read(path, selection, format).unwrap()).unwrap()
}

#[test]
fn word_block_numbers_include_table_rows_and_one_compatibility_branch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("range.docx");
    write_zip(
        &path,
        &[
            ("_rels/.rels", ROOT_RELS_DOCX),
            (
                "word/document.xml",
                r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><w:body>
            <mc:AlternateContent><mc:Choice><w:p><w:r><w:t>duplicate</w:t></w:r></w:p></mc:Choice>
            <mc:Fallback><w:p><w:r><w:t>visible</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent>
            <w:tbl><w:tr><w:tc><w:p><w:r><w:t>项目</w:t></w:r></w:p></w:tc>
            <w:tc><w:p><w:r><w:t>金额</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
            <w:p><w:r><w:t>outside</w:t></w:r></w:p></w:body></w:document>"#,
            ),
        ],
    );
    let result = open(
        &path,
        Selection {
            start: Some(1),
            end: Some(2),
            ..Selection::default()
        },
        "docx",
    );
    assert_eq!(result["count"], 3);
    assert_eq!(
        result["records"],
        json!([
            {"index":1,"kind":"paragraph","text":"visible"},
            {"index":2,"kind":"table_row","table":1,"row":1,"cells":["项目","金额"]},
        ])
    );
}

#[test]
fn long_record_can_be_reassembled_without_losing_unicode_or_escapes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("range.docx");
    let text = "楼层&quot;".repeat(180);
    let xml = format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
        <w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#,
    );
    write_zip(
        &path,
        &[("_rels/.rels", ROOT_RELS_DOCX), ("word/document.xml", &xml)],
    );
    let mut offset = None;
    let mut text = String::new();
    for _ in 0..30 {
        let result = open(
            &path,
            Selection {
                max_bytes: Some(1024),
                offset,
                ..Selection::default()
            },
            "docx",
        );
        assert!(result.to_string().len() <= 1024);
        for fragment in result["records"].as_array().unwrap() {
            text.push_str(fragment["record_json"].as_str().unwrap());
        }
        offset = result["next_offset"].as_u64().map(|next| next as usize);
        if offset.is_none() {
            let record: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(record["text"], "楼层\"".repeat(180));
            return;
        }
    }
    panic!("record continuation never finished");
}

#[test]
fn xlsx_preserves_sparse_addresses_and_expands_shared_formula_from_outside_range() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("range.xlsx");
    write_zip(
        &path,
        &[
            (
                "_rels/.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
                <Relationship Id="main" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
                </Relationships>"#,
            ),
            (
                "[Content_Types].xml",
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
          <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>
          <sheet name="Costs" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>
          <row r="2"><c r="D2"><f t="shared" si="1" ref="D2:D3">B2*$C$1</f><v>12</v></c></row>
          <row r="3"><c r="B3" t="inlineStr"><is><t>000001234567</t></is></c>
          <c r="D3"><f t="shared" si="1"/><v>24</v></c><c r="F3"><f>SUM(A1:A2)</f></c></row>
          </sheetData></worksheet>"#,
            ),
        ],
    );
    let result = open(
        &path,
        Selection {
            sheet: Some("Costs".into()),
            range: Some("$B$3:F3".into()),
            ..Selection::default()
        },
        "xlsx",
    );
    assert_eq!(result["range"], "B3:F3");
    assert_eq!(
        result["records"],
        json!([
            {"address":"B3","value":"000001234567"},
            {"address":"D3","value":24.0,"formula":"=B3*$C$1"},
            {"address":"F3","value":null,"formula":"=SUM(A1:A2)"},
        ])
    );
}

#[test]
fn oversized_shared_string_declaration_is_rejected_before_opening_workbook() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large.xlsx");
    write_zip(
        &path,
        &[(
            "xl/sharedStrings.xml",
            r#"<sst uniqueCount="90000000"></sst>"#,
        )],
    );
    let error = read(&path, Selection::default(), "xlsx").unwrap_err();
    assert!(format!("{error:#}").contains("XLSX shared string count exceeds"));
}
