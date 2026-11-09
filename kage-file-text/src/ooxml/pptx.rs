//! PPTX (EXTENSION_FILE_TEXT §3.7, §5.9).
//!
//! Slides in `p:sldIdLst` order, reached through the presentation's
//! relationships, at most `max_slides` of them (then `pages_skipped`).
//! Each visible slide is a page:
//! - It starts with `## ` and the text of its title placeholder (`title`
//!   or `ctrTitle`), or `## Slide N` without one. N counts every slide,
//!   hidden ones too, as PowerPoint does.
//! - Then each shape's text as a block of lines (`a:p` and `a:br` break
//!   lines), in drawing order, groups included. A table is one line per
//!   row, cells split by a tab.
//! - Then the speaker notes (the notes slide's body placeholder) under
//!   `Notes:`.
//!
//! Left out: hidden slides (`show="0"`) and hidden shapes
//! (`hidden_text_skipped` when they hold text); slide number, date,
//! header and footer placeholders; every `mc:AlternateContent` branch but
//! the first; charts, diagrams, comments and media (other parts, never
//! read).
//!
//! `pages` counts every slide in the deck and the visible ones read;
//! `page_starts` has one entry per slide read. Title: `dc:title`, else the
//! first visible slide's title.

mod slide;

use super::package::{secondary, target_of, Package};
use super::xml::{self, Node, Xml};
use crate::normalize::Normalizer;
use crate::sniff::xml_text;
use crate::{Extracted, Failure, Job, Pages, Warning, WorkerErrorCode};
use slide::{Pass, Slide};
use std::collections::HashMap;

/// Reads a PPTX file.
pub fn extract(job: &Job<'_>) -> Result<Extracted, Failure> {
    let mut pkg = Package::open(job.bytes)?;
    let package_rels = secondary(pkg.rels(""))?.unwrap_or_default();
    let main = Package::main_part(&package_rels, "ppt/presentation.xml");
    let presentation = pkg
        .read(&main)?
        .ok_or_else(|| Failure::new(WorkerErrorCode::Malformed, "no presentation part"))?;
    let max_slides = job.max_slides as usize;
    let (ids, count) = slide_ids(&xml_text(&presentation)?, max_slides)?;
    drop(presentation);
    let rels = secondary(pkg.rels(&main))?.unwrap_or_default();
    let slides: HashMap<&str, &str> = rels
        .iter()
        .filter(|r| r.kind == "slide")
        .map(|r| (r.id.as_str(), r.target.as_str()))
        .collect();

    let mut out = Normalizer::new(job.max_chars);
    let mut hidden = false;
    let mut read = 0u32;
    let mut first_title: Option<Option<String>> = None;
    for (i, id) in ids.iter().enumerate() {
        if out.is_full() {
            break;
        }
        let Some(&part) = slides.get(id.as_str()) else {
            continue;
        };
        let Some(bytes) = pkg.read(part)? else {
            continue;
        };
        let text = xml_text(&bytes)?;
        if slide_hidden(&text)? {
            hidden |= Slide::new(&mut out, Pass::Hidden).walk(&text)?.hidden;
            continue;
        }
        out.start_page();
        read += 1;
        let title = Slide::new(&mut out, Pass::Title).walk(&text)?;
        if !title.started {
            out.push(&format!("## Slide {}", i + 1));
        }
        first_title.get_or_insert_with(|| Some(title.title).filter(|t| !t.is_empty()));
        hidden |= Slide::new(&mut out, Pass::Body).walk(&text)?.hidden;
        hidden |= notes(&mut pkg, part, &mut out)?;
    }

    let skipped = count > max_slides;
    let mut warnings = Vec::new();
    if skipped {
        warnings.push(Warning::PagesSkipped);
    }
    if hidden {
        warnings.push(Warning::HiddenTextSkipped);
    }
    let title = pkg.core_title(&package_rels)?.or(first_title.flatten());
    Ok(Extracted {
        text: out.finish(!skipped),
        title,
        pages: Some(Pages {
            count: u32::try_from(count).unwrap_or(u32::MAX),
            read,
        }),
        warnings,
    })
}

/// The speaker notes of a slide. Best effort: a damaged notes part adds
/// nothing. Returns whether hidden text was left out.
fn notes(pkg: &mut Package<'_>, slide: &str, out: &mut Normalizer) -> Result<bool, Failure> {
    if out.is_full() {
        return Ok(false);
    }
    let Some(rels) = secondary(pkg.rels(slide))? else {
        return Ok(false);
    };
    let Some(part) = target_of(&rels, "notesslide") else {
        return Ok(false);
    };
    let Some(Some(bytes)) = secondary(pkg.read(part))? else {
        return Ok(false);
    };
    let Some(text) = secondary(xml_text(&bytes))? else {
        return Ok(false);
    };
    let found = secondary(Slide::new(out, Pass::Notes).walk(&text))?;
    Ok(found.is_some_and(|f| f.hidden))
}

#[derive(Clone, Copy, PartialEq)]
enum ListTag {
    SldIdLst,
    SldId,
    Other,
}

/// The relationship ids of the first `max` slides, in order, and how
/// many slides the deck lists.
fn slide_ids(text: &str, max: usize) -> Result<(Vec<String>, usize), Failure> {
    let mut xml = Xml::new(text, |local| match local {
        b"sldIdLst" => ListTag::SldIdLst,
        b"sldId" => ListTag::SldId,
        _ => ListTag::Other,
    });
    let (mut ids, mut count) = (Vec::new(), 0usize);
    loop {
        match xml.next()? {
            Node::Open(ListTag::SldId, e) if xml.at(&[ListTag::SldIdLst, ListTag::SldId]) => {
                count += 1;
                if ids.len() < max {
                    // Keep the place of a slide without an id: it still counts.
                    ids.push(xml::prefixed_attr(&e, b"id").unwrap_or_default());
                }
            }
            Node::Eof => return Ok((ids, count)),
            _ => {}
        }
    }
}

/// `show="0"` on the slide's root element.
fn slide_hidden(text: &str) -> Result<bool, Failure> {
    let mut xml = Xml::new(text, |_| ());
    loop {
        match xml.next()? {
            Node::Open((), e) => {
                return Ok(xml::attr(&e, b"show").is_some_and(|v| !xml::on_off(Some(&v))));
            }
            Node::Eof => return Ok(false),
            _ => {}
        }
    }
}
