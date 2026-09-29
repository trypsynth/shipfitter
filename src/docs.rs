//! Markdown documentation turned into standalone HTML pages, such as a readme shipped beside the
//! executable or embedded for a Help menu.
//!
//! The first level 1 heading is the page title. A table of contents of the headings under it
//! follows that heading, and every heading gets an `id` its entry links to, in the same form as
//! GitHub's, so a `[link](#some-heading)` written for GitHub works in the page too. HTML comments
//! in the Markdown are dropped, and any other raw HTML is kept.

use std::{
	collections::VecDeque,
	fmt::Write as _,
	fs,
	path::{Path, PathBuf},
};

use comrak::{
	Anchorizer, Arena, Options, create_formatter,
	html::ChildRendering,
	nodes::{AstNode, NodeValue},
	parse_document,
};

use crate::Result;

/// How a page is rendered.
#[derive(Debug, Clone, Copy)]
pub struct Page<'a> {
	/// The document language, as a BCP 47 tag such as `pt-BR`, so screen readers read the page in
	/// the right voice. [`bcp47`] converts a gettext locale code.
	pub lang: &'a str,
	/// The title when the document has no level 1 heading.
	pub title: &'a str,
	/// How many levels of headings under the title the table of contents lists. Zero leaves it out.
	pub toc_depth: u8,
	/// Whether to include the built-in stylesheet.
	pub style: bool,
	/// HTML added to the end of `<head>`, after the built-in stylesheet, so its rules win.
	pub head: &'a str,
	/// HTML added to the start of `<body>`, before the document.
	pub before_body: &'a str,
}

impl Default for Page<'_> {
	fn default() -> Self {
		Self { lang: "en", title: "Documentation", toc_depth: 3, style: true, head: "", before_body: "" }
	}
}

/// A readme found by [`readmes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readme {
	/// The language code from the file name, as it's written there (`pt_br`, `zh_CN`), or `en`
	/// for `readme.md`.
	pub code: String,
	pub path: PathBuf,
}

impl Readme {
	/// The file name for the HTML page: `readme.html` for English and `readme-<code>.html` for
	/// the rest.
	#[must_use]
	pub fn html_name(&self) -> String {
		if self.code == "en" { "readme.html".to_string() } else { format!("readme-{}.html", self.code) }
	}
}

/// `readme.md` and every `readme-<code>.md` in `dir`: English first, then the rest by code.
pub fn readmes(dir: &Path) -> Result<Vec<Readme>> {
	let mut found = Vec::new();
	for entry in fs::read_dir(dir)? {
		let path = entry?.path();
		if path.extension().and_then(|e| e.to_str()) != Some("md") {
			continue;
		}
		let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
		let code = match stem.strip_prefix("readme-") {
			Some(code) if !code.is_empty() => code.to_string(),
			_ if stem == "readme" => "en".to_string(),
			_ => continue,
		};
		found.push(Readme { code, path });
	}
	found.sort_by(|a, b| (a.code != "en", &a.code).cmp(&(b.code != "en", &b.code)));
	Ok(found)
}

/// A gettext locale code such as `zh_CN` as the BCP 47 tag HTML wants, `zh-CN`.
#[must_use]
pub fn bcp47(code: &str) -> String {
	code.replace('_', "-")
}

/// Converts the Markdown file at `input` into a page at `output`.
///
/// A build script should print `cargo:rerun-if-changed` for the folder the Markdown is in, so
/// adding a translation reruns it too.
pub fn convert(input: &Path, output: &Path, page: &Page) -> Result<()> {
	let markdown = fs::read_to_string(input).map_err(|e| format!("couldn't read {}: {e}", input.display()))?;
	fs::write(output, markdown_to_html(&markdown, page))
		.map_err(|e| format!("couldn't write {}: {e}", output.display()))?;
	Ok(())
}

/// Converts `markdown` into a standalone HTML page.
#[must_use]
pub fn markdown_to_html(markdown: &str, page: &Page) -> String {
	let mut options = Options::default();
	options.extension.strikethrough = true;
	options.extension.table = true;
	options.extension.tasklist = true;
	options.extension.footnotes = true;
	options.parse.smart = true;
	// The documents are the app's own, and may use HTML Markdown can't express.
	options.render.r#unsafe = true;
	let arena = Arena::new();
	let root = parse_document(&arena, markdown, &options);
	remove_comments(root);
	let headings = headings(root);
	let title = headings.iter().find(|h| h.level == 1).map_or(page.title, |h| h.text.as_str());
	let toc = table_of_contents(&headings, page.toc_depth);
	let mut body = String::new();
	// Writing to a String can't fail.
	let unplaced_toc = Formatter::format_document(root, &options, &mut body, toc).unwrap_or_default();
	let mut html = String::new();
	let _ = write!(html, "<!DOCTYPE html>\n<html lang=\"{}\">\n<head>\n<meta charset=\"utf-8\">\n", escape(page.lang));
	html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
	let _ = writeln!(html, "<title>{}</title>", escape(title));
	if page.style {
		html.push_str(STYLE);
	}
	html.push_str(page.head);
	html.push_str("</head>\n<body>\n");
	html.push_str(page.before_body);
	html.push_str("<main>\n");
	// Without a level 1 heading to follow, the table of contents leads the document.
	html.push_str(&unplaced_toc);
	html.push_str(&body);
	html.push_str("</main>\n</body>\n</html>\n");
	html
}

create_formatter!(Formatter<String>, {
	NodeValue::Heading(ref heading) => |context, node, entering| {
		if entering {
			context.cr()?;
			let id = context.anchorizer.anchorize(&node.collect_text());
			write!(context, "<h{} id=\"{id}\">", heading.level)?;
		} else {
			write!(context, "</h{}>", heading.level)?;
			context.lf()?;
			if heading.level == 1 && !context.user.is_empty() {
				let toc = std::mem::take(&mut context.user);
				context.write_str(&toc)?;
			}
		}
		return Ok(ChildRendering::HTML);
	},
});

/// Drops HTML comments, which are notes for whoever edits the Markdown, like the source hash a
/// machine translation records.
fn remove_comments<'a>(root: &'a AstNode<'a>) {
	let comments: Vec<_> = root
		.descendants()
		.filter(|node| match &node.data().value {
			NodeValue::HtmlBlock(block) => is_comment(&block.literal),
			NodeValue::HtmlInline(html) => is_comment(html),
			_ => false,
		})
		.collect();
	for node in comments {
		node.detach();
	}
}

fn is_comment(html: &str) -> bool {
	let html = html.trim();
	html.starts_with("<!--") && html.ends_with("-->")
}

struct Heading {
	level: u8,
	text: String,
	id: String,
}

/// Every heading in document order, with the `id` the formatter will give it: the same
/// [`Anchorizer`] over the same headings in the same order comes up with the same ones.
fn headings<'a>(root: &'a AstNode<'a>) -> Vec<Heading> {
	let mut anchorizer = Anchorizer::new();
	root.descendants()
		.filter_map(|node| match node.data().value {
			NodeValue::Heading(heading) => {
				let text = node.collect_text();
				Some(Heading { level: heading.level, id: anchorizer.anchorize(&text), text })
			}
			_ => None,
		})
		.collect()
}

/// The headings from level 2 down `depth` levels, as nested lists of links.
fn table_of_contents(headings: &[Heading], depth: u8) -> String {
	if depth == 0 {
		return String::new();
	}
	let deepest = depth.saturating_add(1);
	let entries: Vec<_> = headings.iter().filter(|h| (2..=deepest).contains(&h.level)).collect();
	if entries.is_empty() {
		return String::new();
	}
	let mut html = String::from("<nav id=\"TOC\">\n");
	// The levels of the lists currently open. A heading that skips a level, such as a level 4
	// straight under a level 2, nests one list deeper rather than two.
	let mut open: VecDeque<u8> = VecDeque::new();
	for entry in entries {
		while open.back().is_some_and(|&level| level > entry.level) {
			open.pop_back();
			html.push_str("</li>\n</ul>\n");
		}
		if open.back() == Some(&entry.level) {
			html.push_str("</li>\n");
		} else {
			open.push_back(entry.level);
			html.push_str("<ul>\n");
		}
		let _ = write!(html, "<li><a href=\"#{}\">{}</a>", entry.id, escape(&entry.text));
	}
	for _ in open {
		html.push_str("</li>\n</ul>\n");
	}
	html.push_str("</nav>\n");
	html
}

fn escape(text: &str) -> String {
	let mut escaped = String::with_capacity(text.len());
	// Writing to a String can't fail.
	let _ = comrak::html::escape(&mut escaped, text);
	escaped
}

const STYLE: &str = r"<style>
:root { color-scheme: light dark; }
body { font-family: system-ui, sans-serif; line-height: 1.5; max-width: 45em; margin: 0 auto; padding: 1em; }
h1, h2, h3, h4 { line-height: 1.25; }
code { font-family: ui-monospace, Consolas, monospace; font-size: 0.95em; }
pre { overflow-x: auto; padding: 0.75em; border: 1px solid color-mix(in srgb, currentColor 25%, transparent); }
table { border-collapse: collapse; margin: 1em 0; }
th, td { border: 1px solid color-mix(in srgb, currentColor 35%, transparent); padding: 0.3em 0.6em; text-align: left; }
#TOC ul { padding-left: 1.5em; }
</style>
";

#[cfg(test)]
mod tests {
	use super::*;

	fn body(markdown: &str) -> String {
		let html = markdown_to_html(markdown, &Page { style: false, ..Page::default() });
		let start = html.find("<main>\n").unwrap() + "<main>\n".len();
		html[start..html.find("</main>").unwrap()].to_string()
	}

	#[test]
	fn title_comes_from_the_first_level_1_heading() {
		let html = markdown_to_html("# Fedra User Manual\n\nHi.\n", &Page::default());
		assert!(html.contains("<title>Fedra User Manual</title>"));
		assert!(html.contains("<html lang=\"en\">"));
	}

	#[test]
	fn falls_back_to_the_page_title() {
		let html = markdown_to_html("## Only a section\n", &Page { title: "Help & Such", ..Page::default() });
		assert!(html.contains("<title>Help &amp; Such</title>"));
	}

	#[test]
	fn toc_follows_the_title_and_links_to_heading_ids() {
		let html = body("# Manual\n\nIntro.\n\n## Getting Started\n\n### First Run\n\n## Keys\n");
		assert_eq!(
			html,
			"<h1 id=\"manual\">Manual</h1>\n<nav id=\"TOC\">\n<ul>\n\
			 <li><a href=\"#getting-started\">Getting Started</a><ul>\n\
			 <li><a href=\"#first-run\">First Run</a></li>\n</ul>\n</li>\n\
			 <li><a href=\"#keys\">Keys</a></li>\n</ul>\n</nav>\n\
			 <p>Intro.</p>\n<h2 id=\"getting-started\">Getting Started</h2>\n\
			 <h3 id=\"first-run\">First Run</h3>\n<h2 id=\"keys\">Keys</h2>\n"
		);
	}

	#[test]
	fn toc_depth_limits_levels() {
		let html = markdown_to_html("# T\n\n## A\n\n### B\n\n#### C\n", &Page { toc_depth: 2, ..Page::default() });
		assert!(html.contains("href=\"#b\""));
		assert!(!html.contains("href=\"#c\""));
		let html = markdown_to_html("# T\n\n## A\n", &Page { toc_depth: 0, ..Page::default() });
		assert!(!html.contains("<nav"));
	}

	#[test]
	fn duplicate_headings_get_distinct_ids_matching_the_toc() {
		let html = body("# T\n\n## Keys\n\n## Keys\n");
		assert!(html.contains("href=\"#keys-1\""));
		assert!(html.contains("<h2 id=\"keys-1\">"));
	}

	#[test]
	fn skipped_levels_nest_once() {
		let html = body("# T\n\n## A\n\n#### Deep\n\n## B\n");
		assert!(html.contains("<ul>\n<li><a href=\"#a\">A</a><ul>\n<li><a href=\"#deep\">Deep</a></li>\n</ul>\n</li>\n<li><a href=\"#b\">B</a></li>\n</ul>"));
	}

	#[test]
	fn comments_are_dropped_and_other_html_kept() {
		let html = body("<!-- source-hash: abc -->\n\n# T\n\nText <!-- note --> <kbd>Ctrl</kbd>.\n");
		assert!(!html.contains("<!--"));
		assert!(html.contains("<kbd>Ctrl</kbd>"));
	}

	#[test]
	fn heading_markup_is_escaped_in_toc_and_title() {
		let html = markdown_to_html("# A <b>&</b> B\n\n## `x < y`\n", &Page::default());
		assert!(html.contains("<title>A &amp; B</title>"));
		assert!(html.contains(">x &lt; y</a>"));
	}

	#[test]
	fn readmes_lists_english_first() {
		let dir = std::env::temp_dir().join(format!("shipfitter-readmes-{}", std::process::id()));
		fs::create_dir_all(&dir).unwrap();
		for name in ["readme-zh_CN.md", "readme.md", "readme-de.md", "notes.md", "readme-.md"] {
			fs::write(dir.join(name), "").unwrap();
		}
		let codes: Vec<_> = readmes(&dir).unwrap().into_iter().map(|r| r.code).collect();
		fs::remove_dir_all(&dir).unwrap();
		assert_eq!(codes, ["en", "de", "zh_CN"]);
	}
}
