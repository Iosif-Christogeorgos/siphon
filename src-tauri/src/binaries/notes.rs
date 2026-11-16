//! Boils a tool's release notes down to one line for the "updated" toast.

use super::Tool;

/// The longest summary that still reads as one line in a toast.
const MAX_LEN: usize = 110;

/// One plain sentence about what a release changed.
pub fn summarize(tool: Tool, notes: Option<&str>) -> String {
    let notes = notes.unwrap_or_default();
    let summary = match tool {
        // FFmpeg-Builds are nightly snapshots without notes.
        Tool::Ffmpeg => None,
        Tool::YtDlp => fixed_sites(notes).or_else(|| first_bullet(notes)),
        Tool::Deno => first_bullet(notes),
    };
    summary.unwrap_or_else(|| match tool {
        Tool::YtDlp => "Keeps downloads working as sites change.".to_string(),
        Tool::Ffmpeg => "A newer build of the converter.".to_string(),
        Tool::Deno => "A newer version of the script runtime.".to_string(),
    })
}

/// yt-dlp lists what changed per site under "Extractor changes":
///
/// ```text
/// #### Extractor changes
/// - **youtube**: Fix signature extraction (#123) by someone
/// - **tiktok**
///     - Fix …
/// ```
///
/// → "Fixes for youtube, tiktok and 3 more sites."
fn fixed_sites(notes: &str) -> Option<String> {
    let mut sites: Vec<&str> = Vec::new();
    let section = notes
        .lines()
        .skip_while(|line| !line.contains("Extractor changes"))
        .skip(1);
    for line in section.take_while(|line| !line.starts_with('#')) {
        // Top-level bullets only: indented ones are details of the site above.
        let Some(name) = line.strip_prefix("- **").and_then(|rest| rest.split("**").next()) else {
            continue;
        };
        // "youtube:tab" and "youtube" are the same site to a reader.
        let name = name.split(':').next().unwrap_or(name).trim();
        if !name.is_empty() && !sites.contains(&name) {
            sites.push(name);
        }
    }
    let shown = sites.iter().take(3).copied().collect::<Vec<_>>();
    match (shown.as_slice(), sites.len() - shown.len()) {
        ([], _) => None,
        ([one], 0) => Some(format!("Fixes for {one}.")),
        ([rest @ .., last], 0) => Some(format!("Fixes for {} and {last}.", rest.join(", "))),
        (shown, 1) => Some(format!("Fixes for {} and 1 more site.", shown.join(", "))),
        (shown, more) => Some(format!("Fixes for {} and {more} more sites.", shown.join(", "))),
    }
}

/// The first bullet point, with its markdown and attribution stripped.
fn first_bullet(notes: &str) -> Option<String> {
    let bullet = notes
        .lines()
        .find_map(|line| line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")))?;
    let text = plain(bullet);
    // Drop trailing references: "(#1234)", "by someone".
    let text = text.split(" (#").next().unwrap_or(&text);
    let text = text.split(" by ").next().unwrap_or(text).trim().trim_end_matches('.');
    if text.is_empty() {
        return None;
    }
    let mut text: String = text.chars().take(MAX_LEN).collect();
    if text.chars().count() == MAX_LEN {
        text.push('…');
    } else {
        text.push('.');
    }
    // Sentence case, whatever the changelog used.
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
}

/// Removes markdown emphasis and turns `[text](url)` into `text`.
fn plain(markdown: &str) -> String {
    let mut out = String::new();
    let mut rest = markdown;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after
            .find("](")
            .and_then(|close| after[close..].find(')').map(|end| (close, close + end)))
        {
            Some((close, end)) => {
                out.push_str(&after[..close]);
                rest = &after[end + 1..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    const YT_DLP: &str = "#### Important changes\n- **Something important**\n\n#### Core changes\n- Fix a thing ([#1](https://x)) by [me](https://y)\n\n#### Extractor changes\n- **youtube**: Fix signature extraction ([#2](https://x)) by [a](https://y)\n- **youtube:tab**: Fix playlists\n- **tiktok**\n    - Fix **something** nested\n- **twitch**: Fix clips\n- **vimeo**: Fix login\n- **bilibili**: Fix formats\n\n#### Misc. changes\n- **build**: Bump things\n";

    #[test]
    fn yt_dlp_notes_become_a_list_of_fixed_sites() {
        assert_eq!(
            summarize(Tool::YtDlp, Some(YT_DLP)),
            "Fixes for youtube, tiktok, twitch and 2 more sites."
        );
    }

    #[test]
    fn few_sites_are_listed_in_full() {
        let notes = "#### Extractor changes\n- **youtube**: Fix x\n- **tiktok**: Fix y\n";
        assert_eq!(summarize(Tool::YtDlp, Some(notes)), "Fixes for youtube and tiktok.");
        let one = "#### Extractor changes\n- **youtube**: Fix x\n";
        assert_eq!(summarize(Tool::YtDlp, Some(one)), "Fixes for youtube.");
    }

    #[test]
    fn other_notes_use_their_first_bullet_without_markup() {
        let notes = "## What's changed\n- feat(ext/node): support `node:sqlite` backups ([#31234](https://x)) by @someone\n- fix: more\n";
        assert_eq!(
            summarize(Tool::Deno, Some(notes)),
            "Feat(ext/node): support node:sqlite backups."
        );
    }

    #[test]
    fn missing_notes_fall_back_to_a_generic_line() {
        assert_eq!(
            summarize(Tool::Ffmpeg, Some("anything")),
            "A newer build of the converter."
        );
        assert_eq!(summarize(Tool::YtDlp, None), "Keeps downloads working as sites change.");
    }
}
