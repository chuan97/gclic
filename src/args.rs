//! Splitting a command line into its arguments, as a shell would, for `:git`
//! and for an editor's command.

use anyhow::Result;

/// Splits a command line into arguments, as a shell would for simple cases:
/// spaces separate them, and quotes keep spaces in. Inside double quotes, and
/// outside quotes, a backslash only escapes a quote, a space or another
/// backslash, so Windows paths keep theirs.
pub fn split_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    // Whether an argument has started, so `""` makes an empty one.
    let mut started = false;
    let mut quote = None;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => current.push(ch),
            (_, '\\') if matches!(chars.peek(), Some(next) if next.is_whitespace() || matches!(next, '\'' | '"' | '\\')) =>
            {
                current.extend(chars.next());
                started = true;
            }
            (Some(_), _) => current.push(ch),
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (None, _) if ch.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, _) => {
                current.push(ch);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("A quote isn't closed".to_string());
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(line: &str) -> Vec<String> {
        split_args(line).unwrap()
    }

    #[test]
    fn arguments_split_on_spaces_and_keep_quoted_ones_whole() {
        assert_eq!(
            split("commit -m \"fix the  bug\""),
            ["commit", "-m", "fix the  bug"]
        );
        assert_eq!(split("log  --format='%h %s'"), ["log", "--format=%h %s"]);
        assert_eq!(split("commit -m \"\""), ["commit", "-m", ""]);
        assert_eq!(
            split(r#"commit -m "say \"hi\"""#),
            ["commit", "-m", "say \"hi\""]
        );
        assert_eq!(split(r"add my\ file"), ["add", "my file"]);
        assert!(split_args("commit -m \"open").is_err());
    }

    #[test]
    fn windows_paths_keep_their_backslashes() {
        assert_eq!(split(r"add src\main.rs"), ["add", r"src\main.rs"]);
    }
}
