use std::{
    collections::HashMap,
    io::{Read, Seek},
    ops::RangeInclusive,
};

use anyhow::{Context, Result};
use roxmltree::Node;
use serde_json::json;

use super::{Package, Page, parse, tagged};

fn word_tag(node: Node<'_, '_>, name: &str) -> bool {
    tagged(node, "wordprocessingml", name)
}

fn presentation_tag(node: Node<'_, '_>, name: &str) -> bool {
    tagged(node, "presentationml", name)
}

fn drawing_tag(node: Node<'_, '_>, name: &str) -> bool {
    tagged(node, "drawingml", name)
}

fn active_markup(node: Node<'_, '_>) -> bool {
    const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
    for branch in node.ancestors() {
        let Some(container) = branch
            .parent()
            .filter(|parent| parent.has_tag_name((MC, "AlternateContent")))
        else {
            continue;
        };
        let active = container
            .children()
            .find(|child| child.has_tag_name((MC, "Fallback")))
            .or_else(|| {
                container
                    .children()
                    .find(|child| child.has_tag_name((MC, "Choice")))
            });
        if active != Some(branch) {
            return false;
        }
    }
    true
}

fn word_text(paragraph: Node<'_, '_>) -> String {
    let mut output = String::new();
    for node in paragraph.descendants().filter(|node| node.is_element()) {
        if !active_markup(node)
            || node
                .ancestors()
                .take_while(|parent| *parent != paragraph)
                .any(|parent| {
                    word_tag(parent, "del") || word_tag(parent, "moveFrom") || word_tag(parent, "p")
                })
        {
            continue;
        }
        if word_tag(node, "t") {
            output.push_str(node.text().unwrap_or_default());
        } else if word_tag(node, "tab") {
            output.push('\t');
        } else if word_tag(node, "br") || word_tag(node, "cr") {
            output.push('\n');
        }
    }
    output
}

pub(super) fn word(
    package: &mut Package<impl Read + Seek>,
    part: &str,
    selection: RangeInclusive<usize>,
    page: &mut Page,
) -> Result<usize> {
    let xml = package.xml_text(part)?;
    let document = parse(&xml)?;
    let body = document
        .root_element()
        .children()
        .find(|node| word_tag(*node, "body"))
        .context("DOCX document body is missing")?;
    let blocks = body.descendants().filter(|node| {
        (word_tag(*node, "p") || word_tag(*node, "tbl"))
            && active_markup(*node)
            && !node
                .ancestors()
                .skip(1)
                .take_while(|parent| *parent != body)
                .any(|parent| {
                    ["p", "tbl", "del", "moveFrom"]
                        .iter()
                        .any(|name| word_tag(parent, name))
                })
    });
    let mut index = 0;
    let mut table = 0;
    let mut more = true;
    for block in blocks {
        if word_tag(block, "p") {
            index += 1;
            if more && selection.contains(&index) {
                more = page
                    .push(json!({"index": index, "kind": "paragraph", "text": word_text(block)}))?;
            }
        } else {
            table += 1;
            let rows = block.descendants().filter(|node| {
                word_tag(*node, "tr")
                    && active_markup(*node)
                    && !node
                        .ancestors()
                        .skip(1)
                        .take_while(|parent| *parent != block)
                        .any(|parent| word_tag(parent, "tbl") || word_tag(parent, "del"))
            });
            for (row_index, row) in rows.enumerate() {
                index += 1;
                if !more || !selection.contains(&index) {
                    continue;
                }
                let cells: Vec<_> = row
                    .children()
                    .filter(|node| word_tag(*node, "tc"))
                    .map(|cell| {
                        cell.descendants()
                            .filter(|node| word_tag(*node, "p") && active_markup(*node))
                            .map(word_text)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .collect();
                more = page.push(json!({
                    "index": index, "kind": "table_row", "table": table, "row": row_index + 1, "cells": cells,
                }))?;
            }
        }
    }
    Ok(index)
}

fn paragraphs(tree: Node<'_, '_>) -> Vec<String> {
    tree.descendants()
        .filter(|node| drawing_tag(*node, "p") && active_markup(*node))
        .filter_map(|paragraph| {
            let mut text = String::new();
            for node in paragraph.descendants().filter(|node| active_markup(*node)) {
                if drawing_tag(node, "t") {
                    text.push_str(node.text().unwrap_or_default());
                } else if drawing_tag(node, "br") {
                    text.push('\n');
                }
            }
            (!text.is_empty()).then_some(text)
        })
        .collect()
}

fn notes(package: &mut Package<impl Read + Seek>, slide: &str) -> Result<Vec<String>> {
    let Some(part) = package.related(slide, "notesSlide")? else {
        return Ok(Vec::new());
    };
    let xml = package.xml_text(&part)?;
    let document = parse(&xml)?;
    Ok(document
        .descendants()
        .filter(|node| {
            presentation_tag(*node, "sp")
                && active_markup(*node)
                && node.descendants().any(|child| {
                    presentation_tag(child, "ph") && child.attribute("type") == Some("body")
                })
        })
        .flat_map(paragraphs)
        .collect())
}

pub(super) fn slides(
    package: &mut Package<impl Read + Seek>,
    part: &str,
    selection: RangeInclusive<usize>,
    page: &mut Page,
) -> Result<usize> {
    let xml = package.xml_text(part)?;
    let document = parse(&xml)?;
    let Some(list) = document
        .root_element()
        .children()
        .find(|node| presentation_tag(*node, "sldIdLst"))
    else {
        return Ok(0);
    };
    let slides: Vec<_> = list
        .children()
        .filter(|node| presentation_tag(*node, "sldId"))
        .collect();
    let targets: HashMap<_, _> = package.relations(part, "slide")?.into_iter().collect();
    for (index, node) in slides
        .iter()
        .enumerate()
        .map(|(index, node)| (index + 1, node))
    {
        if !selection.contains(&index) {
            continue;
        }
        let id = node
            .attributes()
            .find(|attribute| {
                attribute.name() == "id"
                    && attribute
                        .namespace()
                        .is_some_and(|value| value.ends_with("/relationships"))
            })
            .context("presentation slide missing relationship Id")?
            .value();
        let slide = targets
            .get(id)
            .with_context(|| format!("missing relationship for slide {index}"))?;
        let slide_xml = package.xml_text(slide)?;
        let slide_document = parse(&slide_xml)?;
        let text = slide_document
            .descendants()
            .find(|node| presentation_tag(*node, "spTree"))
            .map(paragraphs)
            .unwrap_or_default();
        if !page.push(json!({"index": index, "text": text, "notes": notes(package, slide)?}))? {
            break;
        }
    }
    Ok(slides.len())
}
