//! Native Single File Component (`.vue`) block extraction and coordinate mapping.
//!
//! Avoids loading an external WebAssembly Vue grammar by splitting `.vue` files
//! into `<template>`, `<script>`, and `<style>` regions so `omp-ast` can analyze
//! each block with its compiled-in tree-sitter grammar (`typescript`, `tsx`,
//! `javascript`, `html`, `css`) and map line/column/byte spans back to the SFC.

use crate::language::SupportLang;

/// High-level kind of a top-level Vue Single File Component tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SfcTagKind {
	Template,
	Script,
	Style,
	Custom,
}

/// One top-level SFC block inside a `.vue` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SfcBlock {
	pub kind:              SfcTagKind,
	/// Normalized lowercase tag name (`"template"`, `"script"`, `"style"`, or custom block tag).
	pub tag_name:          String,
	/// Parsed `lang` attribute value, lowercased if present.
	pub lang:              Option<String>,
	/// Whether the opening tag carries a `setup` attribute (`<script setup>`).
	pub is_setup:          bool,
	/// Byte offset of `<tag` in the original SFC.
	pub outer_start_byte:  usize,
	/// Byte offset immediately after `</tag>` in the original SFC.
	pub outer_end_byte:    usize,
	/// Byte offset immediately after the opening `>` (start of inner slice).
	pub inner_start_byte:  usize,
	/// Byte offset immediately before `</tag>` (end of inner slice).
	pub inner_end_byte:    usize,
	/// 1-based line number of `<tag`.
	pub outer_start_line:  u32,
	/// 1-based line number of `</tag>`.
	pub outer_end_line:    u32,
	/// Number of newlines before `inner_start_byte`; add to 1-based or 0-based sub-lines.
	pub inner_line_offset: u32,
	/// Byte column offset of `inner_start_byte` within its own line.
	pub inner_col_offset:  usize,
}

impl SfcBlock {
	/// Slice of the inner content belonging to this block.
	#[must_use]
	pub fn inner_source<'a>(&self, source: &'a str) -> &'a str {
		&source[self.inner_start_byte..self.inner_end_byte]
	}

	/// Native tree-sitter script grammar corresponding to this block's `lang` attribute.
	#[must_use]
	pub fn script_lang(&self) -> SupportLang {
		match self.lang.as_deref() {
			Some("ts" | "typescript" | "mts" | "cts") => SupportLang::TypeScript,
			Some("tsx") => SupportLang::Tsx,
			_ => SupportLang::JavaScript,
		}
	}

	/// Native tree-sitter template grammar (`Html` for standard HTML/Vue templates;
	/// `None` for custom preprocessors like Pug).
	#[must_use]
	pub fn template_lang(&self) -> Option<SupportLang> {
		match self.lang.as_deref() {
			None | Some("html") => Some(SupportLang::Html),
			_ => None,
		}
	}

	/// Native tree-sitter style grammar (`Css` for plain CSS; `None` for SCSS/Less/etc.).
	#[must_use]
	pub fn style_lang(&self) -> Option<SupportLang> {
		match self.lang.as_deref() {
			None | Some("css") => Some(SupportLang::Css),
			_ => None,
		}
	}

	/// Native tree-sitter grammar for the interior of this SFC block, if supported natively.
	#[must_use]
	pub fn content_lang(&self) -> Option<SupportLang> {
		match self.kind {
			SfcTagKind::Script => Some(self.script_lang()),
			SfcTagKind::Template => self.template_lang(),
			SfcTagKind::Style => self.style_lang(),
			SfcTagKind::Custom => None,
		}
	}
}

/// Extract top-level blocks (`<script>`, `<template>`, `<style>`, custom) from a `.vue` file.
#[must_use]
pub fn parse_sfc_blocks(source: &str) -> Vec<SfcBlock> {
	let bytes = source.as_bytes();
	let mut blocks = Vec::new();
	let mut i = 0;

	while i < bytes.len() {
		if bytes[i] != b'<' {
			i += 1;
			continue;
		}
		if bytes[i..].starts_with(b"<!--") {
			i = skip_html_comment(bytes, i + 4);
			continue;
		}
		if bytes[i..].starts_with(b"</") || bytes[i..].starts_with(b"<!") || bytes[i..].starts_with(b"<?") {
			i += 2;
			continue;
		}

		let Some(open) = parse_open_tag(source, i) else {
			i += 1;
			continue;
		};
		if open.self_closing {
			i = open.end_byte;
			continue;
		}

		let kind = match open.tag_name.as_str() {
			"template" => SfcTagKind::Template,
			"script" => SfcTagKind::Script,
			"style" => SfcTagKind::Style,
			_ => SfcTagKind::Custom,
		};

		let found_close = match kind {
			SfcTagKind::Template => find_matching_template_close(source, open.end_byte),
			SfcTagKind::Script => find_raw_tag_close(source, open.end_byte, "script", true),
			SfcTagKind::Style => find_raw_tag_close(source, open.end_byte, "style", false),
			SfcTagKind::Custom => find_raw_tag_close(source, open.end_byte, &open.tag_name, false),
		};

		let Some((inner_end_byte, outer_end_byte)) = found_close else {
			i = open.end_byte;
			continue;
		};

		let outer_start_byte = open.start_byte;
		let inner_start_byte = open.end_byte;
		let outer_start_line = count_newlines(&bytes[..outer_start_byte]) + 1;
		let outer_end_line = count_newlines(&bytes[..inner_end_byte]) + 1;
		let inner_line_offset = count_newlines(&bytes[..inner_start_byte]);
		let line_start = bytes[..inner_start_byte]
			.iter()
			.rposition(|&b| b == b'\n')
			.map_or(0, |pos| pos + 1);
		let inner_col_offset = inner_start_byte - line_start;

		blocks.push(SfcBlock {
			kind,
			tag_name: open.tag_name,
			lang: open.lang,
			is_setup: open.is_setup,
			outer_start_byte,
			outer_end_byte,
			inner_start_byte,
			inner_end_byte,
			outer_start_line,
			outer_end_line,
			inner_line_offset,
			inner_col_offset,
		});

		i = outer_end_byte;
	}

	blocks
}

/// Find the SFC block containing a 1-based source line.
#[must_use]
pub fn block_at_line(blocks: &[SfcBlock], line: u32) -> Option<&SfcBlock> {
	blocks
		.iter()
		.find(|b| line >= b.outer_start_line && line <= b.outer_end_line)
}

#[derive(Debug)]
struct OpenTag {
	tag_name:     String,
	lang:         Option<String>,
	is_setup:     bool,
	self_closing: bool,
	start_byte:   usize,
	end_byte:     usize,
}

fn parse_open_tag(source: &str, start_byte: usize) -> Option<OpenTag> {
	let bytes = source.as_bytes();
	if start_byte + 1 >= bytes.len() || bytes[start_byte] != b'<' {
		return None;
	}
	let name_start = start_byte + 1;
	if !bytes[name_start].is_ascii_alphabetic() {
		return None;
	}
	let mut pos = name_start + 1;
	while pos < bytes.len() && is_tag_name_byte(bytes[pos]) {
		pos += 1;
	}
	let tag_name = source[name_start..pos].to_ascii_lowercase();
	let attr_start = pos;

	// Must be followed by ASCII whitespace, `>`, or `/>`.
	if pos < bytes.len() && !bytes[pos].is_ascii_whitespace() && bytes[pos] != b'>' && !bytes[pos..].starts_with(b"/>") {
		return None;
	}

	let mut quote: Option<u8> = None;
	while pos < bytes.len() {
		let b = bytes[pos];
		if let Some(q) = quote {
			if b == q {
				quote = None;
			}
			pos += 1;
			continue;
		}
		if b == b'"' || b == b'\'' {
			quote = Some(b);
			pos += 1;
			continue;
		}
		if b == b'>' {
			let prev_non_ws = bytes[attr_start..pos]
				.iter()
				.rposition(|c| !c.is_ascii_whitespace())
				.map(|rel| attr_start + rel);
			let self_closing = prev_non_ws.is_some_and(|idx| bytes[idx] == b'/');
			let attr_end = if self_closing {
				prev_non_ws.unwrap_or(pos)
			} else {
				pos
			};
			let (lang, is_setup) = parse_tag_attributes(&source[attr_start..attr_end]);
			return Some(OpenTag {
				tag_name,
				lang,
				is_setup,
				self_closing,
				start_byte,
				end_byte: pos + 1,
			});
		}
		pos += 1;
	}
	None
}

fn parse_tag_attributes(attrs: &str) -> (Option<String>, bool) {
	let bytes = attrs.as_bytes();
	let mut i = 0;
	let mut lang = None;
	let mut is_setup = false;

	while i < bytes.len() {
		while i < bytes.len() && bytes[i].is_ascii_whitespace() {
			i += 1;
		}
		if i >= bytes.len() {
			break;
		}
		let name_start = i;
		while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
			i += 1;
		}
		let attr_name = &attrs[name_start..i];
		while i < bytes.len() && bytes[i].is_ascii_whitespace() {
			i += 1;
		}
		let mut attr_val: Option<&str> = None;
		if i < bytes.len() && bytes[i] == b'=' {
			i += 1;
			while i < bytes.len() && bytes[i].is_ascii_whitespace() {
				i += 1;
			}
			if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
				let q = bytes[i];
				i += 1;
				let val_start = i;
				while i < bytes.len() && bytes[i] != q {
					i += 1;
				}
				attr_val = Some(&attrs[val_start..i]);
				if i < bytes.len() && bytes[i] == q {
					i += 1;
				}
			} else {
				let val_start = i;
				while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
					i += 1;
				}
				attr_val = Some(&attrs[val_start..i]);
			}
		}

		if attr_name.eq_ignore_ascii_case("setup") {
			is_setup = attr_val.is_none_or(|v| !v.eq_ignore_ascii_case("false"));
		} else if attr_name.eq_ignore_ascii_case("lang")
			&& let Some(v) = attr_val
		{
			let trimmed = v.trim();
			if !trimmed.is_empty() {
				lang = Some(trimmed.to_ascii_lowercase());
			}
		}
	}

	(lang, is_setup)
}

fn find_matching_template_close(source: &str, start: usize) -> Option<(usize, usize)> {
	let bytes = source.as_bytes();
	let mut depth = 1usize;
	let mut i = start;

	while i < bytes.len() {
		if bytes[i] != b'<' {
			i += 1;
			continue;
		}
		if bytes[i..].starts_with(b"<!--") {
			i = skip_html_comment(bytes, i + 4);
			continue;
		}
		if let Some(close_end) = match_closing_tag(bytes, i, "template") {
			depth -= 1;
			if depth == 0 {
				return Some((i, close_end));
			}
			i = close_end;
			continue;
		}
		if i + 1 < bytes.len()
			&& bytes[i + 1].is_ascii_alphabetic()
			&& let Some(open) = parse_open_tag(source, i)
		{
			if open.tag_name == "template" && !open.self_closing {
				depth += 1;
			}
			i = open.end_byte;
			continue;
		}
		i += 1;
	}
	None
}

fn find_raw_tag_close(
	source: &str,
	start: usize,
	tag_name: &str,
	is_script: bool,
) -> Option<(usize, usize)> {
	let bytes = source.as_bytes();
	let mut i = start;

	while i < bytes.len() {
		let b = bytes[i];
		// Skip comments & string literals so embedded "</script>" strings don't prematurely terminate.
		if b == b'/' && i + 1 < bytes.len() {
			if is_script && bytes[i + 1] == b'/' {
				i += 2;
				while i < bytes.len() && bytes[i] != b'\n' {
					i += 1;
				}
				continue;
			}
			if bytes[i + 1] == b'*' {
				i += 2;
				while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
					i += 1;
				}
				i = (i + 2).min(bytes.len());
				continue;
			}
		}
		if b == b'\'' || b == b'"' || (is_script && b == b'`') {
			let quote = b;
			let multiline = quote == b'`';
			i += 1;
			while i < bytes.len() {
				let cur = bytes[i];
				if cur == b'\\' {
					i = (i + 2).min(bytes.len());
					continue;
				}
				if !multiline && cur == b'\n' {
					break;
				}
				if cur == quote {
					i += 1;
					break;
				}
				i += 1;
			}
			continue;
		}
		if b == b'<'
			&& let Some(close_end) = match_closing_tag(bytes, i, tag_name)
		{
			return Some((i, close_end));
		}
		i += 1;
	}
	None
}

fn match_closing_tag(bytes: &[u8], pos: usize, tag_name: &str) -> Option<usize> {
	if pos + 2 >= bytes.len() || bytes[pos] != b'<' || bytes[pos + 1] != b'/' {
		return None;
	}
	let tag_bytes = tag_name.as_bytes();
	let name_start = pos + 2;
	let name_end = name_start + tag_bytes.len();
	if name_end > bytes.len() || !bytes[name_start..name_end].eq_ignore_ascii_case(tag_bytes) {
		return None;
	}
	let mut j = name_end;
	if j < bytes.len() && is_tag_name_byte(bytes[j]) {
		return None;
	}
	while j < bytes.len() && bytes[j].is_ascii_whitespace() {
		j += 1;
	}
	(bytes.get(j) == Some(&b'>')).then_some(j + 1)
}

fn skip_html_comment(bytes: &[u8], mut i: usize) -> usize {
	while i + 2 < bytes.len() {
		if bytes[i..].starts_with(b"-->") {
			return i + 3;
		}
		i += 1;
	}
	bytes.len()
}

const fn is_tag_name_byte(b: u8) -> bool {
	b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b':'
}

fn count_newlines(slice: &[u8]) -> u32 {
	slice.iter().filter(|&&b| b == b'\n').count().min(u32::MAX as usize) as u32
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_multiline_generic_script_nested_template_and_scss_style() {
		let src = r#"<!-- leading comment -->
<script
  setup
  lang="ts"
  generic="T extends Record<string, any>"
>
import { ref } from 'vue';
const literal = '</script>';
// comment </script>
function helper() {
  return 1;
}
</script>

<template>
  <Table>
    <template #cell="{ row }">
      <span>{{ row.name }}</span>
    </template>
  </Table>
</template>

<style scoped lang="scss">
.box { color: red; }
</style>
"#;
		let blocks = parse_sfc_blocks(src);
		assert_eq!(blocks.len(), 3);

		let script = &blocks[0];
		assert_eq!(script.kind, SfcTagKind::Script);
		assert_eq!(script.lang.as_deref(), Some("ts"));
		assert!(script.is_setup);
		assert_eq!(script.script_lang(), SupportLang::TypeScript);
		assert_eq!(script.outer_start_line, 2);
		assert_eq!(script.outer_end_line, 13);
		assert_eq!(script.inner_line_offset, 5);
		assert!(script.inner_source(src).contains("const literal = '</script>';"));

		let template = &blocks[1];
		assert_eq!(template.kind, SfcTagKind::Template);
		assert_eq!(template.outer_start_line, 15);
		assert_eq!(template.outer_end_line, 21);
		assert!(template.inner_source(src).contains("<template #cell="));

		let style = &blocks[2];
		assert_eq!(style.kind, SfcTagKind::Style);
		assert_eq!(style.lang.as_deref(), Some("scss"));
		assert_eq!(style.style_lang(), None);
		assert_eq!(style.outer_start_line, 23);
		assert_eq!(style.outer_end_line, 25);
	}

	#[test]
	fn template_attr_containing_closing_tag_literal_does_not_terminate_early() {
		let src = r#"<template>
  <Input :placeholder="cond ? '</template>' : 'ok'" />
  <template v-if="1 < 2">
    <span>nested</span>
  </template>
</template>
"#;
		let blocks = parse_sfc_blocks(src);
		assert_eq!(blocks.len(), 1);
		assert_eq!(blocks[0].outer_start_line, 1);
		assert_eq!(blocks[0].outer_end_line, 6);
	}

	#[test]
	fn parses_multiple_script_blocks_and_defaults_to_javascript() {
		let src = "<script>\nexport const A = 1;\n</script>\n<script setup lang=\"ts\">\nconst b = 2;\n</script>\n";
		let blocks = parse_sfc_blocks(src);
		assert_eq!(blocks.len(), 2);
		assert!(!blocks[0].is_setup);
		assert_eq!(blocks[0].script_lang(), SupportLang::JavaScript);
		assert!(blocks[1].is_setup);
		assert_eq!(blocks[1].script_lang(), SupportLang::TypeScript);
	}
}
