//! Human-friendly CLI output formatters.
//!
//! Each `fmt_*` function formats one tool's output for terminal display.
//! When `color` is true, ANSI escape codes are emitted via `owo_colors`.

use crate::tools::{
    ContextOutput, DiffOutput, GetOutput, IndexOutput, OutlineOutput, RefsOutput, SearchOutput,
    StatsOutput, TocOutput,
};
use owo_colors::OwoColorize;
use std::io::{self, Write};

// ── search ──────────────────────────────────────────────────────────────────

/// Expands compact source chars to human-readable labels for CLI display.
fn expand_sources(compact: &str) -> String {
    compact
        .chars()
        .filter_map(|c| match c {
            'f' => Some("fts"),
            'g' => Some("grep"),
            't' => Some("trigram"),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("+")
}

pub fn fmt_search(w: &mut impl Write, out: &SearchOutput, color: bool) -> io::Result<()> {
    for item in &out.results {
        // Path + score + sources
        let sources = expand_sources(&item.sources);
        if color {
            writeln!(
                w,
                "{}  ({:.2} · {})",
                item.path.bold(),
                item.score,
                sources.dimmed()
            )?;
        } else {
            writeln!(w, "{}  ({:.2} · {})", item.path, item.score, sources)?;
        }

        // Snippets
        for s in &item.snippets {
            if color {
                writeln!(w, "  {}{}", format_args!("{:>5}│ ", s.line).green(), s.text)?;
            } else {
                writeln!(w, "  {:>5}│ {}", s.line, s.text)?;
            }
        }
    }

    if out.has_more {
        if color {
            writeln!(w, "{}", "... more results available".dimmed())?;
        } else {
            writeln!(w, "... more results available")?;
        }
    }

    Ok(())
}

// ── refs ────────────────────────────────────────────────────────────────────

pub fn fmt_refs(w: &mut impl Write, out: &RefsOutput, color: bool) -> io::Result<()> {
    let mut current_path = "";

    for r in &out.references {
        // Group header when path changes
        if r.path != current_path {
            if !current_path.is_empty() {
                writeln!(w)?;
            }
            if color {
                writeln!(w, "{}", r.path.bold())?;
            } else {
                writeln!(w, "{}", r.path)?;
            }
            current_path = &r.path;
        }

        // Reference line
        if color {
            let ref_type_colored = match r.ref_type.as_str() {
                "definition" => format!("{:<12}", r.ref_type).green().to_string(),
                "import" => format!("{:<12}", r.ref_type).blue().to_string(),
                "type_usage" => format!("{:<12}", r.ref_type).yellow().to_string(),
                _ => format!("{:<12}", r.ref_type),
            };
            writeln!(
                w,
                "  {} {}{}",
                ref_type_colored,
                format_args!("{:>5}│ ", r.line).dimmed(),
                r.content
            )?;
        } else {
            writeln!(w, "  {:<12} {:>5}│ {}", r.ref_type, r.line, r.content)?;
        }
    }

    Ok(())
}

// ── outline ─────────────────────────────────────────────────────────────────

pub fn fmt_outline(w: &mut impl Write, out: &OutlineOutput, color: bool) -> io::Result<()> {
    if color {
        writeln!(w, "{} ({})", out.path.bold(), out.file_type.dimmed())?;
    } else {
        writeln!(w, "{} ({})", out.path, out.file_type)?;
    }

    for sym in &out.symbols {
        let indent = "  ".repeat(sym.level);
        let end_info = sym.end_line.map(|e| format!("-{e}")).unwrap_or_default();

        if color {
            let kind_colored = match sym.kind.as_str() {
                "fn" => format!("{:<6}", "fn").blue().to_string(),
                "struct" => format!("{:<6}", "struct").green().to_string(),
                "enum" => format!("{:<6}", "enum").yellow().to_string(),
                "impl" => format!("{:<6}", "impl").cyan().to_string(),
                "trait" => format!("{:<6}", "trait").magenta().to_string(),
                "class" => format!("{:<6}", "class").green().to_string(),
                "mod" => format!("{:<6}", "mod").cyan().to_string(),
                "iface" => format!("{:<6}", "iface").magenta().to_string(),
                other => format!("{other:<6}"),
            };
            writeln!(
                w,
                "  {indent}{kind_colored} {:<20} :{}{}",
                sym.name, sym.line, end_info
            )?;
        } else {
            writeln!(
                w,
                "  {indent}{:<6} {:<20} :{}{}",
                sym.kind, sym.name, sym.line, end_info
            )?;
        }
    }

    Ok(())
}

// ── toc ─────────────────────────────────────────────────────────────────────

pub fn fmt_toc(w: &mut impl Write, out: &TocOutput) -> io::Result<()> {
    write!(w, "{}", out.tree)?;
    writeln!(
        w,
        "{} directories, {} files",
        out.total_dirs, out.total_files
    )?;
    Ok(())
}

// ── context ─────────────────────────────────────────────────────────────────

pub fn fmt_context(w: &mut impl Write, out: &ContextOutput, color: bool) -> io::Result<()> {
    if color {
        writeln!(w, "{}:{}", out.path.bold(), out.center_line)?;
    } else {
        writeln!(w, "{}:{}", out.path, out.center_line)?;
    }

    // Strip content boundary markers and print formatted lines
    for line in out.content.lines() {
        if line.starts_with("--- BEGIN FILE CONTENT:") || line.starts_with("--- END FILE CONTENT:")
        {
            continue;
        }

        let is_center = line.starts_with('>');

        if color && is_center {
            writeln!(w, "{}", line.bold())?;
        } else {
            writeln!(w, "{line}")?;
        }
    }

    Ok(())
}

// ── diff ────────────────────────────────────────────────────────────────────

pub fn fmt_diff(w: &mut impl Write, out: &DiffOutput, color: bool) -> io::Result<()> {
    for hunk in &out.hunks {
        for line in hunk.content.lines() {
            if color {
                if line.starts_with('+') {
                    writeln!(w, "{}", line.green())?;
                } else if line.starts_with('-') {
                    writeln!(w, "{}", line.red())?;
                } else if line.starts_with("@@") {
                    writeln!(w, "{}", line.cyan())?;
                } else {
                    writeln!(w, "{line}")?;
                }
            } else {
                writeln!(w, "{line}")?;
            }
        }
    }

    if out.truncated {
        writeln!(w, "[output truncated — full diff has more hunks]")?;
    }

    writeln!(
        w,
        "{} additions, {} deletions",
        out.stats.additions, out.stats.deletions
    )?;

    Ok(())
}

// ── stats ───────────────────────────────────────────────────────────────────

pub fn fmt_stats(w: &mut impl Write, out: &StatsOutput, color: bool) -> io::Result<()> {
    if color {
        writeln!(w, "{:<16} {}", "Files:".bold(), out.total_files)?;
        writeln!(w, "{:<16} {}", "Trigrams:".bold(), out.trigram_count)?;
        writeln!(
            w,
            "{:<16} {} ({})",
            "Index size:".bold(),
            out.index_size.human,
            out.index_size.bytes
        )?;
    } else {
        writeln!(w, "{:<16} {}", "Files:", out.total_files)?;
        writeln!(w, "{:<16} {}", "Trigrams:", out.trigram_count)?;
        writeln!(
            w,
            "{:<16} {} ({})",
            "Index size:", out.index_size.human, out.index_size.bytes
        )?;
    }

    if let Some(by_type) = &out.by_type {
        writeln!(w)?;
        if color {
            writeln!(w, "{}", "By file type:".bold())?;
        } else {
            writeln!(w, "By file type:")?;
        }

        let mut types: Vec<_> = by_type.iter().collect();
        types.sort_by(|a, b| b.1.cmp(a.1));
        for (ext, count) in types {
            writeln!(w, "  .{ext:<12} {count}")?;
        }
    }

    Ok(())
}

// ── index ───────────────────────────────────────────────────────────────────

pub fn fmt_index(w: &mut impl Write, out: &IndexOutput) -> io::Result<()> {
    writeln!(w, "{}", out.message)?;
    Ok(())
}

// ── get ─────────────────────────────────────────────────────────────────────

pub fn fmt_get(w: &mut impl Write, out: &GetOutput) -> io::Result<()> {
    // Strip content boundary markers, print raw content
    for line in out.content.lines() {
        if line.starts_with("--- BEGIN FILE CONTENT:") || line.starts_with("--- END FILE CONTENT:")
        {
            continue;
        }
        writeln!(w, "{line}")?;
    }
    Ok(())
}
