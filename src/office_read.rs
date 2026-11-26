use std::{
    fs::{self, File},
    io::{Read, Seek},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use rmcp::schemars::{self, JsonSchema};
use roxmltree::{Document, Node, ParsingOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zip::ZipArchive;

mod open_xml;
#[cfg(test)]
mod tests;
mod workbook;

pub(crate) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    /// Absolute directory containing the source file.
    pub(crate) root: PathBuf,
    /// Relative DOCX, XLS, XLSX, PPT or PPTX file.
    pub(crate) file: PathBuf,
    /// Excel sheet name. Omit it to list sheets.
    sheet: Option<String>,
    /// Excel A1 cell or rectangular range. Defaults to A1:J20.
    range: Option<String>,
    /// First DOCX body block or PowerPoint slide, inclusive and one-based.
    start: Option<usize>,
    /// Last DOCX body block or PowerPoint slide, inclusive. Read up to 50 at a time.
    end: Option<usize>,
    /// Pass next_offset from the previous response with the same selection and max_bytes.
    offset: Option<usize>,
    /// JSON output budget in bytes, 1,024 to 65,536. Defaults to 8,000.
    max_bytes: Option<usize>,
}

pub(crate) fn read(path: &Path, selection: Selection, source_format: &str) -> Result<String> {
    let size = path.metadata()?.len();
    ensure!(
        size <= MAX_FILE_BYTES,
        "Office file exceeds the 64 MiB input limit"
    );
    let mut page = Page::new(&selection)?;
    let extension = extension(path);
    let mut index = match extension.as_str() {
        "xlsx" => {
            ensure!(
                selection.start.is_none() && selection.end.is_none(),
                "use sheet and range for Excel"
            );
            workbook::read(path, source_format, &selection, &mut page)?
        }
        "docx" | "pptx" => {
            ensure!(
                selection.sheet.is_none() && selection.range.is_none(),
                "use start and end for Word or PowerPoint"
            );
            let start = selection.start.unwrap_or(1);
            let end = selection.end.unwrap_or_else(|| start.saturating_add(14));
            ensure!(
                start > 0 && end >= start && end - start < 50,
                "provide an inclusive range of 1 to 50 blocks or slides"
            );
            let mut package = Package(ZipArchive::new(File::open(path)?)?);
            let main = package
                .related("", "officeDocument")?
                .context("missing Office document part")?;
            let (key, count) = match extension.as_str() {
                "docx" => (
                    "blocks",
                    open_xml::word(&mut package, &main, start..=end, &mut page)?,
                ),
                _ => (
                    "slides",
                    open_xml::slides(&mut package, &main, start..=end, &mut page)?,
                ),
            };
            json!({"format": source_format, "unit": key, "count": count, "start": start, "end": end.min(count)})
        }
        _ => bail!("unsupported Office extension: .{extension}; use DOCX, XLS/XLSX or PPT/PPTX"),
    };
    if extension != source_format {
        index["converted_to"] = json!(extension);
    }
    page.finish(index)
}

pub(crate) fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

pub(crate) fn legacy_conversion(
    path: &Path,
    source: &str,
    target: &str,
    output: &Path,
) -> Result<(tokio::process::Command, PathBuf)> {
    ensure!(
        path.metadata()?.len() <= MAX_FILE_BYTES,
        "Office file exceeds the 64 MiB input limit"
    );
    let input = output.join(format!("source.{source}"));
    fs::copy(path, &input).context("copy legacy Office file to temporary directory")?;
    let profile = output.join("profile");
    fs::create_dir_all(profile.join("user"))?;
    fs::write(
        profile.join("user/registrymodifications.xcu"),
        concat!(
            r#"<oor:items xmlns:oor="http://openoffice.org/2001/registry">"#,
            r#"<item oor:path="/org.openoffice.Office.Common/Security/Scripting">"#,
            r#"<prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop>"#,
            "</item></oor:items>",
        ),
    )?;
    let profile = profile
        .to_str()
        .context("LibreOffice profile path must be Unicode")?;
    let profile = profile.replace('\\', "/");
    const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b':')
        .remove(b'-')
        .remove(b'_')
        .remove(b'.');
    let encoded = profile
        .split('/')
        .map(|part| percent_encoding::utf8_percent_encode(part, PATH_SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/");
    let prefix = if encoded.starts_with('/') {
        "file://"
    } else {
        "file:///"
    };
    let mut command = tokio::process::Command::new(soffice());
    command
        .arg(format!("-env:UserInstallation={prefix}{encoded}"))
        .args([
            "--headless",
            "--norestore",
            "--convert-to",
            target,
            "--outdir",
        ])
        .arg(output)
        .arg(input);
    Ok((command, output.join(format!("source.{target}"))))
}

fn soffice() -> PathBuf {
    let executable = if cfg!(windows) {
        "soffice.com"
    } else {
        "soffice"
    };
    if let Some(paths) = std::env::var_os("PATH")
        && let Some(found) = std::env::split_paths(&paths)
            .map(|dir| dir.join(executable))
            .find(|file| file.is_file())
    {
        return found;
    }
    if cfg!(target_os = "macos") {
        let path = Path::new("/Applications/LibreOffice.app/Contents/MacOS/soffice");
        if path.is_file() {
            return path.to_path_buf();
        }
    }
    if cfg!(windows)
        && let Some(programs) = std::env::var_os("ProgramFiles")
    {
        let path = PathBuf::from(programs)
            .join("LibreOffice")
            .join("program")
            .join(executable);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(executable)
}

struct Page {
    offset: usize,
    seen: usize,
    limit: usize,
    bytes: usize,
    records: Vec<Value>,
    next: Option<usize>,
}

impl Page {
    fn new(selection: &Selection) -> Result<Self> {
        let limit = selection.max_bytes.unwrap_or(8_000);
        ensure!(
            (1_024..=65_536).contains(&limit),
            "max_bytes must be between 1024 and 65536"
        );
        Ok(Self {
            offset: selection.offset.unwrap_or(0),
            seen: 0,
            limit,
            bytes: 0,
            records: Vec::new(),
            next: None,
        })
    }

    /// Returns false once the next record or fragment needs a separate page.
    fn push(&mut self, record: impl Serialize) -> Result<bool> {
        let record = serde_json::to_value(record)?;
        let encoded = serde_json::to_string(&record)?;
        if encoded.len() + 513 <= self.limit {
            return Ok(self.push_item(record, encoded.len() + 1));
        }

        // A fragment is a JSON string inside the response and may escape every byte.
        let chunk_limit = (self.limit - 640) / 2;
        let mut rest = encoded.as_str();
        let mut part = 1;
        while !rest.is_empty() {
            let end = rest.floor_char_boundary(rest.len().min(chunk_limit));
            let (text, remaining) = rest.split_at(end);
            let fragment = json!({"part": part, "last": remaining.is_empty(), "record_json": text});
            let bytes = serde_json::to_vec(&fragment)?.len() + 1;
            if !self.push_item(fragment, bytes) {
                return Ok(false);
            }
            part += 1;
            rest = remaining;
        }
        Ok(true)
    }

    fn push_item(&mut self, record: Value, bytes: usize) -> bool {
        if self.seen < self.offset {
            self.seen += 1;
            return true;
        }
        // Reserve space for the index and paging fields as well as the array.
        if self.bytes + bytes + 512 > self.limit {
            self.next = Some(self.seen);
            return false;
        }
        self.bytes += bytes;
        self.seen += 1;
        self.records.push(record);
        true
    }

    fn finish(self, mut index: Value) -> Result<String> {
        ensure!(
            self.offset <= self.seen,
            "offset is past the selected records"
        );
        index["records"] = Value::Array(self.records);
        if let Some(next) = self.next {
            index["next_offset"] = json!(next);
        }
        let output = serde_json::to_string(&index)?;
        ensure!(
            output.len() <= self.limit,
            "Office output exceeds max_bytes"
        );
        Ok(output)
    }
}

struct Package<R: Read + Seek>(ZipArchive<R>);

impl<R: Read + Seek> Package<R> {
    fn xml_text(&mut self, part: &str) -> Result<String> {
        const MAX_PART: u64 = 32 * 1024 * 1024;
        let member = self
            .0
            .by_name(part)
            .with_context(|| format!("open Office part {part}"))?;
        ensure!(
            member.size() <= MAX_PART,
            "Office part exceeds 32 MiB: {part}"
        );
        let mut data = Vec::new();
        member.take(MAX_PART + 1).read_to_end(&mut data)?;
        ensure!(
            data.len() as u64 <= MAX_PART,
            "Office part exceeds 32 MiB: {part}"
        );
        if data.starts_with(&[0xff, 0xfe]) || data.starts_with(&[0xfe, 0xff]) {
            let little_endian = data[0] == 0xff;
            ensure!(data.len() % 2 == 0, "odd-length UTF-16 Office part: {part}");
            let units = data[2..]
                .chunks_exact(2)
                .map(|pair| {
                    let pair = [pair[0], pair[1]];
                    if little_endian {
                        u16::from_le_bytes(pair)
                    } else {
                        u16::from_be_bytes(pair)
                    }
                })
                .collect::<Vec<_>>();
            return String::from_utf16(&units).context("decode UTF-16 Office XML");
        }
        String::from_utf8(data).context("decode UTF-8 Office XML")
    }

    fn relations(&mut self, owner: &str, kind: &str) -> Result<Vec<(String, String)>> {
        let member = if owner.is_empty() {
            "_rels/.rels".to_owned()
        } else {
            let (dir, name) = owner.rsplit_once('/').unwrap_or(("", owner));
            format!(
                "{}_rels/{name}.rels",
                if dir.is_empty() {
                    String::new()
                } else {
                    format!("{dir}/")
                }
            )
        };
        if self.0.index_for_name(&member).is_none() {
            return Ok(Vec::new());
        }
        let text = self.xml_text(&member)?;
        let document = parse(&text)?;
        let suffix = format!("/{kind}");
        document
            .root_element()
            .children()
            .filter(|node| {
                node.has_tag_name("Relationship")
                    && node
                        .attribute("Type")
                        .is_some_and(|value| value.ends_with(&suffix))
                    && node.attribute("TargetMode") != Some("External")
            })
            .map(|node| {
                let id = node.attribute("Id").context("relationship missing Id")?;
                let target = node
                    .attribute("Target")
                    .context("relationship missing Target")?;
                Ok((id.to_owned(), resolve_part(owner, target)?))
            })
            .collect()
    }

    fn related(&mut self, owner: &str, kind: &str) -> Result<Option<String>> {
        Ok(self
            .relations(owner, kind)?
            .into_iter()
            .next()
            .map(|(_, target)| target))
    }
}

fn parse(text: &str) -> Result<Document<'_>> {
    Document::parse_with_options(
        text,
        ParsingOptions {
            nodes_limit: 1_000_000,
            ..ParsingOptions::default()
        },
    )
    .context("parse Office XML")
}

fn resolve_part(owner: &str, target: &str) -> Result<String> {
    let target = percent_encoding::percent_decode_str(target).decode_utf8()?;
    ensure!(
        !target.contains(['\\', ':', '#', '?', '\0']),
        "invalid Office part target"
    );
    let mut path = Vec::new();
    if !target.starts_with('/')
        && let Some((dir, _)) = owner.rsplit_once('/')
    {
        path.extend(dir.split('/'));
    }
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                ensure!(path.pop().is_some(), "Office part target escapes package");
            }
            name => path.push(name),
        }
    }
    ensure!(!path.is_empty(), "empty Office part target");
    Ok(path.join("/"))
}

fn tagged(node: Node<'_, '_>, kind: &str, name: &str) -> bool {
    node.has_tag_name(name)
        && node.tag_name().namespace().is_some_and(|namespace| {
            namespace
                .strip_prefix("http://schemas.openxmlformats.org/")
                .is_some_and(|suffix| suffix.strip_suffix("/2006/main") == Some(kind))
                || namespace
                    .strip_prefix("http://purl.oclc.org/ooxml/")
                    .is_some_and(|suffix| suffix.strip_suffix("/main") == Some(kind))
        })
}
