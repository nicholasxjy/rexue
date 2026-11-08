//! Process boundary: separate arguments, bounded output/deadlines, explicit shell scripts.
use anyhow::{Context, Result, bail};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use wait_timeout::ChildExt;
const MAX_OUTPUT: usize = 16 * 1024 * 1024;

pub fn output(command: &mut Command, timeout: Duration) -> Result<Output> {
    Capture::start(command, timeout, false)?.finish(None)
}

struct Capture {
    child: Option<Child>,
    label: String,
    deadline: Instant,
    output: mpsc::Receiver<(bool, std::io::Result<Vec<u8>>)>,
    prelude: mpsc::Receiver<std::io::Result<Vec<u8>>>,
}

impl Capture {
    fn start(command: &mut Command, timeout: Duration, exchange: bool) -> Result<Self> {
        let label = format!("{:?}", command.get_program());
        command
            .stdin(if exchange {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("could not start {label}; is it installed and on PATH?"))?;
        let deadline = Instant::now() + timeout;
        let stdout = child.stdout.take().context("missing stdout")?;
        let stderr = child.stderr.take().context("missing stderr")?;
        let (tx, rx) = mpsc::channel();
        let (prelude_tx, prelude_rx) = mpsc::channel();
        for (is_stdout, reader) in [
            (true, Box::new(stdout) as Box<dyn Read + Send>),
            (false, Box::new(stderr) as Box<dyn Read + Send>),
        ] {
            let tx = tx.clone();
            let prelude_tx = prelude_tx.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(reader.take((MAX_OUTPUT + 1) as u64));
                let mut consumed = 0;
                if exchange && is_stdout {
                    let mut line = Vec::new();
                    let result = reader.read_until(b'\n', &mut line);
                    consumed = line.len();
                    let _ = prelude_tx.send(result.map(|_| line));
                }
                let mut data = Vec::new();
                let result = reader.read_to_end(&mut data).and_then(|_| {
                    if consumed + data.len() > MAX_OUTPUT {
                        Err(std::io::Error::other("process output exceeded 16 MiB"))
                    } else {
                        Ok(data)
                    }
                });
                let _ = tx.send((is_stdout, result));
            });
        }
        drop(tx);
        Ok(Self {
            child: Some(child),
            label,
            deadline,
            output: rx,
            prelude: prelude_rx,
        })
    }

    fn left(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    fn finish(mut self, reply: Option<Vec<u8>>) -> Result<Output> {
        if let Some(reply) = reply {
            let mut stdin = self
                .child
                .as_mut()
                .unwrap()
                .stdin
                .take()
                .context("missing stdin")?;
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let result = stdin.write_all(&reply).and_then(|_| stdin.write_all(b"\n"));
                let _ = tx.send(result);
            });
            rx.recv_timeout(self.left())
                .with_context(|| format!("{} input timed out", self.label))??;
        }
        let left = self.left();
        let status = match self.child.as_mut().unwrap().wait_timeout(left)? {
            Some(status) => status,
            None => bail!("{} timed out", self.label),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        for _ in 0..2 {
            let (is_stdout, data) = match self.output.recv_timeout(self.left()) {
                Ok(v) => v,
                Err(_) => bail!("{} output timed out", self.label),
            };
            let data = data?;
            if data.len() > MAX_OUTPUT {
                bail!(
                    "{} output exceeded {} MiB",
                    self.label,
                    MAX_OUTPUT / 1024 / 1024
                );
            }
            if is_stdout {
                stdout = data
            } else {
                stderr = data
            }
        }
        let result = Output {
            status,
            stdout,
            stderr,
        };
        self.child.take();
        if !result.status.success() {
            let message = String::from_utf8_lossy(&result.stderr);
            bail!(
                "{} failed (exit {}): {}",
                self.label,
                result.status.code().unwrap_or(130),
                message.trim()
            );
        }
        Ok(result)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            kill_group(child.id(), child);
        }
    }
}

/// One bounded JSON handshake and reply on the same child connection.
pub(crate) struct Exchange(Capture);
impl Exchange {
    pub fn start(command: &mut Command, timeout: Duration) -> Result<(Self, Vec<u8>)> {
        let capture = Capture::start(command, timeout, true)?;
        let prelude = capture
            .prelude
            .recv_timeout(capture.left())
            .with_context(|| format!("{} handshake timed out", capture.label))??;
        if prelude.is_empty() {
            capture.finish(None)?;
            bail!("process exited without a handshake");
        }
        if prelude.len() > MAX_OUTPUT {
            bail!("process handshake exceeded 16 MiB");
        }
        Ok((Self(capture), prelude))
    }

    pub fn finish(self, reply: Vec<u8>) -> Result<Output> {
        self.0.finish(Some(reply))
    }
}
/// Run a Worktrunk mutation with its native hook/approval UI on the terminal.
/// stdout stays machine-readable; a file avoids blocking hooks on a full pipe.
/// No --yes/--no-hooks flags are inserted and stdin is inherited deliberately.
pub fn interactive_text(command: &mut Command) -> Result<String> {
    use std::io::{Seek, SeekFrom};
    let mut stdout = tempfile::tempfile()?;
    let mut child = command
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdout(Stdio::from(stdout.try_clone()?))
        .spawn()
        .context("could not start Worktrunk; is wt installed and on PATH?")?;
    let status = match child.wait_timeout(Duration::from_secs(15 * 60))? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Worktrunk timed out after 15 minutes");
        }
    };
    if !status.success() {
        bail!(
            "Worktrunk failed (exit {}); see diagnostics on stderr",
            status.code().unwrap_or(130)
        );
    }
    if stdout.metadata()?.len() > MAX_OUTPUT as u64 {
        bail!("Worktrunk output exceeded {} MiB", MAX_OUTPUT / 1024 / 1024);
    }
    stdout.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    stdout.read_to_string(&mut text)?;
    Ok(text)
}

fn kill_group(pid: u32, child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // Each command starts its own process group, so this cannot target Rex itself.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
    let _ = child.kill();
    let _ = child.wait();
}
pub fn text(command: &mut Command) -> Result<String> {
    Ok(String::from_utf8_lossy(&output(command, Duration::from_secs(30))?.stdout).into_owned())
}
/// Replace path placeholders using the shell syntax tree, not a flat quote state.
/// The path itself remains a separate positional argument, never shell source.
/// Unsupported/ambiguous templates produce a shell diagnostic and exit status 2;
/// the String API is also used by backend startup commands.
pub fn path_script(script: &str) -> String {
    if !script.contains("{}") {
        return script.into();
    }
    let mut binding = "__REXUE_PATH_VALUE".to_owned();
    while script.contains(&binding) {
        binding.push('_');
    }
    match compile_path_script(script, &binding) {
        Ok(script) => format!("readonly {binding}=\"${{1}}\"\n{script}"),
        Err(error) => format!(
            "printf '%s\\n' {} >&2; exit 2",
            shell_words::quote(&format!(
                "rexue: unsupported shell syntax around path placeholder: {error}"
            )),
        ),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PathQuote {
    Unquoted,
    Double,
    Single,
    Ansi,
    Heredoc,
    Comment,
}

fn path_quote(mut node: tree_sitter::Node<'_>) -> PathQuote {
    loop {
        match node.kind() {
            "string" => return PathQuote::Double,
            // In a double-quoted parameter word, single quotes are literal data.
            // A command substitution stops this inheritance below.
            "raw_string" => {
                return if parameter_word_inherits_quotes(node) {
                    PathQuote::Double
                } else {
                    PathQuote::Single
                };
            }
            "ansi_c_string" => {
                return if parameter_word_inherits_quotes(node) {
                    PathQuote::Double
                } else {
                    PathQuote::Ansi
                };
            }
            "heredoc_body" => return PathQuote::Heredoc,
            "comment" => return PathQuote::Comment,
            "command_substitution" | "process_substitution" => return PathQuote::Unquoted,
            _ => {}
        }
        let Some(parent) = node.parent() else {
            return PathQuote::Unquoted;
        };
        node = parent;
    }
}

fn parameter_word_inherits_quotes(mut node: tree_sitter::Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        node = parent;
        match node.kind() {
            "expansion" => {
                let Some(operator) = node.child_by_field_name("operator") else {
                    return false;
                };
                return matches!(
                    operator.kind(),
                    "-" | ":-" | "+" | ":+" | "=" | ":=" | "?" | ":?"
                ) && path_quote(node) == PathQuote::Double;
            }
            "command_substitution" | "process_substitution" | "string" => return false,
            _ => {}
        }
    }
    false
}

// Validate the argument boundary before a rewrite can hide its ancestry.
// Command substitutions establish a new scope; arithmetic outside them must
// never receive a path, since shells can evaluate parameter data as source.
fn path_context(mut node: tree_sitter::Node<'_>) -> Result<Option<tree_sitter::Node<'_>>> {
    loop {
        match node.kind() {
            // In heredocs the grammar can spell $((...)) as a command
            // substitution containing an immediately adjacent subshell. sh
            // still evaluates arithmetic, so retain the same path boundary.
            "command_substitution"
                if node.named_child(0).is_some_and(|child| {
                    child.kind() == "subshell" && child.start_byte() == node.start_byte() + 2
                }) =>
            {
                bail!("path in arithmetic or variable name")
            }
            "command_substitution" | "process_substitution" => return Ok(None),
            "arithmetic_expansion" | "subscript" | "variable_name" => {
                bail!("path in arithmetic or variable name")
            }
            "heredoc_start" | "heredoc_end" => bail!("unresolved heredoc delimiter"),
            "heredoc_body" => return Ok(Some(node)),
            _ => {}
        }
        let Some(parent) = node.parent() else {
            return Ok(None);
        };
        node = parent;
    }
}

fn compile_path_script(script: &str, binding: &str) -> Result<String> {
    let reference = format!("${{{binding}}}");
    if !script.contains("{}") {
        return Ok(script.into());
    }
    // A word token keeps template braces from being mistaken for shell grammar
    // (notably the closing brace in ${missing:-{}}). Its byte ranges are obtained
    // from this same masked source, including when the original contains UTF-8.
    let mut marker = "__REXUE_PATH__".to_owned();
    while script.contains(&marker) {
        marker.push('_');
    }
    let source = script.replace("{}", &marker);
    let (analysis, mut edits) = multiple_heredoc_edits(&source, &marker, binding)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_bash::LANGUAGE.into())?;
    let tree = parser
        .parse(&analysis, None)
        .context("shell parser stopped")?;
    let root = tree.root_node();
    // Normalize placeholder delimiters before checking parser errors. The
    // grammar's heredoc scanner also accepts a delimiter *prefix* as its end;
    // a body line beginning with {} can otherwise truncate the AST. Use the
    // AST opener and exact complete lines to find the shell's real closing
    // delimiter, then compile again with a fresh constant absent from source.
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if node.kind() == "heredoc_start" && source[node.byte_range()].contains(&marker) {
            let opener = &source[node.byte_range()];
            let words = shell_words::split(opener).context("invalid heredoc delimiter")?;
            let delimiter = words.first().context("empty heredoc delimiter")?;
            let redirect = node.parent().context("missing heredoc redirect")?;
            let strip_tabs = source[redirect.start_byte()..node.start_byte()].contains("<<-");
            let body_start = source[node.end_byte()..]
                .find('\n')
                .map(|i| node.end_byte() + i + 1)
                .context("missing heredoc body")?;
            let mut offset = body_start;
            let mut closing = None;
            for line in source[body_start..].split_inclusive('\n') {
                let content = line.strip_suffix('\n').unwrap_or(line);
                let content = if strip_tabs {
                    content.trim_start_matches('\t')
                } else {
                    content
                };
                if content == delimiter {
                    let tabs = if strip_tabs {
                        line.len() - line.trim_start_matches('\t').len()
                    } else {
                        0
                    };
                    closing = Some(offset + tabs..offset + line.trim_end_matches('\n').len());
                    break;
                }
                offset += line.len();
            }
            let closing = closing.context("missing heredoc end")?;
            let name = heredoc_path_delimiter(&source, "__REXUE_END__", node.start_byte());
            let opener = if opener.contains(['\'', '"', '\\']) {
                format!("'{name}'")
            } else {
                name.clone()
            };
            let normalized = format!(
                "{}{}{}{}{}",
                &source[..node.start_byte()],
                opener,
                &source[node.end_byte()..closing.start],
                name,
                &source[closing.end..],
            );
            return compile_path_script(&normalized.replace(&marker, "{}"), binding);
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break;
            }
        }
        if cursor.node() == root {
            break;
        }
    }
    if root.has_error() {
        bail!("ambiguous shell template");
    }
    // The grammar treats single quotes in parameter words as shell quotes,
    // even when the expansion inherits double quoting and sh treats them as
    // literal characters. Reparse the entire word in that inherited context:
    // this exposes substitutions hidden inside raw_string tokens, while their
    // command bodies get their own independent quote scopes from the AST.
    // Select outermost words only; recursion handles nested parameter defaults.
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if node.kind() == "expansion"
            && path_quote(node) == PathQuote::Double
            && !edits
                .iter()
                .any(|(range, _)| range.contains(&node.start_byte()))
            && let Some(operator) = node.child_by_field_name("operator")
            && matches!(
                &source[operator.byte_range()],
                "-" | ":-" | "+" | ":+" | "=" | ":=" | "?" | ":?" | "#" | "##" | "%" | "%%"
            )
        {
            let closing = node
                .child(node.child_count() - 1)
                .context("missing parameter end")?;
            let range = operator.end_byte()..closing.start_byte();
            let word = &source[range.clone()];
            if word.contains(&marker) {
                for (offset, _) in word.match_indices(&marker) {
                    let start = range.start + offset;
                    let placeholder = root
                        .descendant_for_byte_range(start, start + marker.len())
                        .context("missing parameter placeholder")?;
                    path_context(placeholder)?;
                }
                // Pattern-removal words keep independent quote scopes even
                // inside a double-quoted expansion; default words inherit it.
                let pattern = matches!(&source[operator.byte_range()], "#" | "##" | "%" | "%%");
                let (prefix, suffix) = if pattern {
                    ("printf '%s' ", "")
                } else {
                    ("printf '%s' \"", "\"")
                };
                let wrapped = format!("{prefix}{}{suffix}", word.replace(&marker, "{}"));
                let rewritten = compile_path_script(&wrapped, binding)?;
                let replacement = rewritten
                    .strip_prefix(prefix)
                    .and_then(|s| s.strip_suffix(suffix))
                    .context("parameter word wrapper changed")?;
                // The recursive word compilation includes any heredocs that
                // were already analyzed independently inside this range.
                edits.retain(|(edit, _)| !(range.start <= edit.start && edit.end <= range.end));
                edits.push((range, replacement.into()));
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break;
            }
        }
        if cursor.node() == root {
            break;
        }
    }
    for (start, _) in source.match_indices(&marker) {
        let end = start + marker.len();
        if edits.iter().any(|(range, _)| range.contains(&start)) {
            continue;
        }
        let node = root
            .descendant_for_byte_range(start, end)
            .context("missing placeholder node")?;
        if let Some(body) = path_context(node)? {
            let redirect = body.parent().context("missing heredoc redirect")?;
            let mut cursor = redirect.walk();
            let delimiter = redirect
                .children(&mut cursor)
                .find(|n| n.kind() == "heredoc_start")
                .context("missing heredoc delimiter")?;
            if source[delimiter.byte_range()].contains(['\'', '"', '\\']) {
                let mut cursor = redirect.walk();
                let closing = redirect
                    .children(&mut cursor)
                    .find(|n| n.kind() == "heredoc_end")
                    .context("missing heredoc end")?;
                // Quoted heredocs cannot expand $1. Make the delimiter unquoted
                // and escape every original expansion/backslash in the literal
                // body. The only live expansion is our positional parameter.
                let delimiter_name = heredoc_path_delimiter(&source, &marker, start);
                let literal = &source[body.byte_range()];
                let mut replacement = String::new();
                for (i, part) in literal.split(&marker).enumerate() {
                    if i > 0 {
                        replacement.push_str(&reference);
                    }
                    for c in part.chars() {
                        if matches!(c, '$' | '`' | '\\') {
                            replacement.push('\\');
                        }
                        replacement.push(c);
                    }
                }
                edits.push((delimiter.byte_range(), delimiter_name.clone()));
                edits.push((body.byte_range(), replacement));
                edits.push((closing.byte_range(), delimiter_name));
                continue;
            }
        }
        let quote = path_quote(node);
        // Keep escaped placeholders literal, as before. Single-quoted strings
        // have no backslash escapes; quoted heredocs were handled as literals.
        let backslashes = source[..start]
            .bytes()
            .rev()
            .take_while(|b| *b == b'\\')
            .count();
        if quote == PathQuote::Comment || (quote != PathQuote::Single && backslashes % 2 == 1) {
            continue;
        }
        let replacement = match quote {
            PathQuote::Single => format!("'\"{reference}\"'"),
            PathQuote::Double | PathQuote::Heredoc => reference.clone(),
            PathQuote::Unquoted => format!("\"{reference}\""),
            PathQuote::Ansi => format!("'\"{reference}\"$'"),
            PathQuote::Comment => unreachable!(),
        };
        edits.push((start..end, replacement));
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut result = String::new();
    let mut offset = 0;
    for (range, replacement) in edits {
        if range.start < offset {
            bail!("overlapping shell contexts");
        }
        result.push_str(&source[offset..range.start].replace(&marker, "{}"));
        result.push_str(&replacement);
        offset = range.end;
    }
    result.push_str(&source[offset..].replace(&marker, "{}"));
    Ok(result)
}
type PathEdit = (std::ops::Range<usize>, String);

// tree-sitter-bash supports one heredoc per command. Discover additional
// openers by masking just the preceding opener and reparsing the same bytes.
// Exact delimiter lines, consumed in opener order, establish the real bodies;
// the parser's erroneous body ranges never decide which source gets executed.
fn multiple_heredoc_edits(
    source: &str,
    marker: &str,
    binding: &str,
) -> Result<(String, Vec<PathEdit>)> {
    fn mask(source: &mut [u8], range: std::ops::Range<usize>) {
        for byte in &mut source[range] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    fn openers(tree: &tree_sitter::Tree) -> Vec<std::ops::Range<usize>> {
        let mut result = Vec::new();
        let mut cursor = tree.walk();
        loop {
            let node = cursor.node();
            if node.kind() == "heredoc_start" {
                let redirect = node.parent().unwrap();
                let mut children = redirect.walk();
                if let Some(operator) = redirect
                    .children(&mut children)
                    .find(|n| matches!(n.kind(), "<<" | "<<-"))
                {
                    result.push(operator.start_byte()..node.end_byte());
                }
            }
            if cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    return result;
                }
            }
        }
    }
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_bash::LANGUAGE.into())?;
    let mut analysis = source.as_bytes().to_vec();
    let mut visited = Vec::new();
    let mut edits = Vec::new();
    loop {
        let tree = parser
            .parse(&analysis, None)
            .context("shell parser stopped")?;
        let Some(first) = openers(&tree)
            .into_iter()
            .find(|range| !visited.contains(&range.start))
        else {
            break;
        };
        visited.push(first.start);
        let Some(newline) = source[first.end..].find('\n').map(|i| first.end + i) else {
            continue;
        };
        let mut headers = vec![first.clone()];
        let mut probe = analysis.clone();
        mask(&mut probe, first);
        loop {
            let tree = parser.parse(&probe, None).context("shell parser stopped")?;
            let Some(next) = openers(&tree)
                .into_iter()
                .find(|range| range.start >= headers.last().unwrap().end && range.end <= newline)
            else {
                break;
            };
            mask(&mut probe, next.clone());
            headers.push(next);
        }
        if headers.len() < 2 {
            continue;
        }
        let mut body_start = newline + 1;
        for header in headers {
            let raw = &source[header.clone()];
            let strip_tabs = raw.starts_with("<<-");
            let opener = raw[if strip_tabs { 3 } else { 2 }..].trim_start();
            let words = shell_words::split(opener).context("invalid heredoc delimiter")?;
            let delimiter = words.first().context("empty heredoc delimiter")?;
            let mut offset = body_start;
            let mut closing = None;
            for line in source[body_start..].split_inclusive('\n') {
                let content = line.strip_suffix('\n').unwrap_or(line);
                let content = if strip_tabs {
                    content.trim_start_matches('\t')
                } else {
                    content
                };
                if content == delimiter {
                    closing = Some(offset + line.trim_end_matches('\n').len());
                    break;
                }
                offset += line.len();
            }
            let body_end = closing.context("missing heredoc end")?;
            // A standalone redirect gives each body its true quote context,
            // including substitutions nested inside unquoted heredocs. Recursion
            // handles multi-heredocs inside those substitutions as well.
            let body = &source[body_start..body_end];
            // The grammar can hide substitutions behind <<- indentation.
            // Strip source tabs before analysis, exactly as sh does before
            // expansion; tabs introduced by the positional value stay data.
            let body = if strip_tabs {
                body.split_inclusive('\n')
                    .map(|line| line.trim_start_matches('\t'))
                    .collect::<String>()
            } else {
                body.to_owned()
            };
            let standalone = format!("cat {raw}\n{body}\n");
            let compiled = compile_path_script(&standalone.replace(marker, "{}"), binding)?;
            let (header_text, body_text) =
                compiled.split_once('\n').context("missing heredoc body")?;
            edits.push((
                header.clone(),
                header_text
                    .strip_prefix("cat ")
                    .context("heredoc wrapper changed")?
                    .into(),
            ));
            edits.push((
                body_start..body_end,
                body_text
                    .strip_suffix('\n')
                    .context("heredoc wrapper changed")?
                    .into(),
            ));
            mask(&mut analysis, header);
            mask(&mut analysis, body_start..body_end);
            body_start = body_end + usize::from(source.as_bytes().get(body_end) == Some(&b'\n'));
        }
    }
    Ok((String::from_utf8(analysis)?, edits))
}

fn heredoc_path_delimiter(source: &str, marker: &str, start: usize) -> String {
    let mut name = format!("{marker}HEREDOC_{start}");
    while source.contains(&name) {
        name.push('_');
    }
    name
}

pub fn shell_script(script: &str, path: &std::path::Path) -> Result<String> {
    shell_argument(script, path.as_os_str(), path)
}
pub fn shell_argument(
    script: &str,
    arg: &std::ffi::OsStr,
    cwd: &std::path::Path,
) -> Result<String> {
    text(
        Command::new("sh")
            .args(["-c", &path_script(script), "rexue"])
            .arg(arg)
            .current_dir(cwd),
    )
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_keeps_paths_as_data_in_every_quote_context() {
        let d = tempfile::tempdir().unwrap();
        let arg = std::ffi::OsStr::new("a b'\";$(touch INJECTED)");
        for script in ["printf '%s' {}", "printf '%s' '{}'", "printf '%s' \"{}\""] {
            let text = shell_argument(script, arg, d.path()).unwrap();
            assert_eq!(text, arg.to_string_lossy());
            assert!(!d.path().join("INJECTED").exists());
        }
    }
    #[test]
    fn placeholder_nested_command_substitutions_use_the_inner_quote_context() {
        let d = tempfile::tempdir().unwrap();
        let arg = std::ffi::OsStr::new("a b'\";$(touch INJECTED)");
        for script in [
            r#"printf '%s' "$(printf '%s' '{}')""#,
            r#"printf '%s' "$(printf '%s' {})""#,
            r#"printf '%s' "$(printf '%s' "{}")""#,
            r#"printf '%s' "$(printf '%s' "$(printf '%s' '{}')")""#,
            r#"printf '%s' "`printf '%s' '{}'`""#,
            r#"printf '%s' "$(printf '%s' "`printf '%s' '{}'`")""#,
            r#"printf '%s' "`printf '%s' "$(printf '%s' '{}')"`""#,
        ] {
            let actual = shell_argument(script, arg, d.path()).unwrap();
            assert_eq!(actual, arg.to_string_lossy(), "{script}");
            assert!(!d.path().join("INJECTED").exists(), "{script}");
        }
        assert_eq!(
            shell_argument(
                r#"printf '%s' "$(basename '{}')""#,
                std::ffi::OsStr::new("/Users/nick/Projects/rexue"),
                d.path(),
            )
            .unwrap(),
            "rexue",
        );
    }

    #[test]
    fn placeholder_respects_shell_scopes_and_literal_heredocs() {
        let d = tempfile::tempdir().unwrap();
        let arg = std::ffi::OsStr::new("a b'\";$(touch INJECTED)`touch INJECTED`\\tail");
        for script in [
            r#"printf '%s' "$(case x in (x) printf '%s' '{}';; esac)""#,
            r#"printf '%s' "`case x in x) printf '%s' '{}';; esac`""#,
            r#"printf '%s' "$(rexue_emit() { printf '%s' '{}'; }; rexue_emit "$1")""#,
            r#"printf '%s' "$( (printf '%s' '{}') )""#,
            r#"printf '%s' "$(n=$((1 + (2 * 3))); printf '%s' '{}')""#,
            r#"unset rexue_missing; printf '%s' "${rexue_missing:-{}}""#,
            r#"unset rexue_missing; printf '%s' ${rexue_missing:-'{}'}"#,
            r#"unset rexue_missing; printf '%s' "${rexue_missing:-"{}"}""#,
            r#"unset rexue_missing; : "$(( ${rexue_missing:-$(printf '%s' '{}' > captured; printf 0)} ))"; cat captured"#,
            r#"printf '%s' "$(cat <<EOF
$(printf '%s' '{}')
EOF
)""#,
            "cat <<'EOF'\n{}{}\nEOF\n",
            "cat <<'EOF'\n{}\nEOF\n",
            "cat <<\\EOF\n{}\nEOF\n",
            "cat <<EOF\n{}\nEOF\n",
            "printf '%s' \"$(cat <<'EOF'\n{}\nEOF\n)\"",
        ] {
            let expected = if script == "cat <<'EOF'\n{}{}\nEOF\n" {
                format!("{0}{0}\n", arg.to_string_lossy())
            } else if script.starts_with("cat ") {
                format!("{}\n", arg.to_string_lossy())
            } else {
                arg.to_string_lossy().into_owned()
            };
            assert_eq!(
                shell_argument(script, arg, d.path()).unwrap(),
                expected,
                "{script}"
            );
            assert!(!d.path().join("INJECTED").exists(), "{script}");
        }
        for script in [
            r#"unset rexue_missing; printf '%s' "${rexue_missing:-'{}'}""#,
            r#"unset rexue_missing; printf '%s' "${rexue_missing:-${rexue_missing:-'{}'}}""#,
        ] {
            assert_eq!(
                shell_argument(script, arg, d.path()).unwrap(),
                format!("'{}'", arg.to_string_lossy()),
                "{script}",
            );
        }
        assert_eq!(
            shell_argument(
                "cat <<'EOF'\n'$literal' `literal` \\\n{}\nEOF\n",
                arg,
                d.path()
            )
            .unwrap(),
            format!("'$literal' `literal` \\\n{}\n", arg.to_string_lossy()),
        );
    }

    #[test]
    fn placeholder_preserves_word_boundaries_and_literal_source() {
        let d = tempfile::tempdir().unwrap();
        for arg in [
            "",
            "* ? [abc]",
            "é\\路径\nline'\"$()`touch INJECTED`",
            "__REXUE_PATH__",
        ] {
            for script in [
                r#"printf '<%s>' pre{}post"#,
                r#"printf '<%s>' 'pre{}post'"#,
                r#"printf '<%s>' "pre{}post""#,
                r#"printf '<%s>' "$(printf '%s' pre{}post)""#,
            ] {
                assert_eq!(
                    shell_argument(script, arg.as_ref(), d.path()).unwrap(),
                    format!("<pre{arg}post>"),
                    "{script}",
                );
                assert!(!d.path().join("INJECTED").exists());
            }
        }
        assert_eq!(
            shell_argument(
                r#"printf '%s' \{}; # ignored {}"#,
                "unused".as_ref(),
                d.path()
            )
            .unwrap(),
            "{}",
        );
        let source = "printf '%s' \"$(case x in x) printf '%s' '{}';; esac)\"";
        // macOS sh 3.2 cannot execute a bare case arm inside $(), even
        // without placeholders. Assert its parser context here; the equivalent
        // (x) and backtick forms above execute through actual sh.
        assert_eq!(
            compile_path_script(source, "1").unwrap(),
            "printf '%s' \"$(case x in x) printf '%s' ''\"${1}\"'';; esac)\"",
        );
    }

    #[test]
    fn placeholder_hidden_parameter_command_substitution_runs_in_sh() {
        let d = tempfile::tempdir().unwrap();
        let arg = "a b'\";$(touch INJECTED)`touch INJECTED`\\路径";
        let script =
            r#"unset rexue_missing; printf '%s' "${rexue_missing:-'$(printf '%s' '{}')'}""#;
        assert_eq!(
            shell_argument(script, arg.as_ref(), d.path()).unwrap(),
            format!("'{arg}'"),
        );
        assert!(!d.path().join("INJECTED").exists());
    }

    #[test]
    fn placeholder_hidden_parameter_words_preserve_sh_semantics() {
        let d = tempfile::tempdir().unwrap();
        for body in [
            r#"printf '%s' "${rexue_missing:-'$(printf '%s' '{}')'}""#,
            r#"printf '%s' "${rexue_missing:-'$(printf '%s' {})'}""#,
            r#"printf '%s' "${rexue_missing:-'$(printf '%s' "{}")'}""#,
            r#"printf '%s' "${rexue_missing:-'$(printf '%s' "\"{}\"")'}""#,
            r#"printf '%s' "${rexue_missing:-'$(printf '%s' "$(printf '%s' '{}')")'}""#,
            r#"printf '%s' "${rexue_missing:-'`printf '%s' '{}'`'}""#,
            r#"printf '%s' "${rexue_missing:-'${rexue_missing:-'$(printf '%s' '{}')'}'}""#,
            r#"printf '%s' "${rexue_missing:-'$(printf '%s' "${rexue_missing:-'$(printf '%s' '{}')'}")'}""#,
            r#"printf '%s' "${rexue_missing:-'$(case x in (x) printf '%s' '{}';; esac)'}""#,
            r#"printf '%s' "${rexue_missing:-'$( (printf '%s' '{}') )'}""#,
            r#"printf '%s' "${rexue_missing:-'$(n=$((1 + (2 * 3))); printf '%s' '{}')'}""#,
            r#"printf '%s' "${rexue_missing:-'$(printf '%s' '{}')'}${rexue_missing:-'$(printf '%s' '{}')'}""#,
            r#"printf '%s' "${rexue_missing:-'pre{}$(printf '%s' '{}'){}post'}""#,
            r#"printf '%s' "${rexue_missing:-"pre{}post"}""#,
            r#"printf '%s' "${rexue_missing-'$(printf '%s' '{}')'}""#,
            r#"printf '%s' "${rexue_missing:='$(printf '%s' '{}')'}" "$rexue_missing""#,
            r#"printf '%s' "${rexue_missing='$(printf '%s' '{}')'}" "$rexue_missing""#,
            r#"rexue_missing=yes; printf '%s' "${rexue_missing:+'$(printf '%s' '{}')'}""#,
            r#"rexue_missing=yes; printf '%s' "${rexue_missing+'$(printf '%s' '{}')'}""#,
            r#"rexue_missing=yes; printf '%s' "${rexue_missing:-'$(touch UNSELECTED; printf '%s' '{}')'}""#,
            "printf '%s' \"${rexue_missing:-'$(cat <<EOF\n$(printf '%s' '{}')\nEOF\n)'}\"",
            "printf '%s' \"${rexue_missing:-'$(cat <<'EOF'\n{}\nEOF\n)'}\"",
        ] {
            let script = format!("unset rexue_missing; {body}");
            // Let the actual sh establish the surrounding literal quote/word
            // semantics independently, using an inert placeholder value.
            let reference = script.replace("{}", "REX_EXPECTED_PATH");
            let expected = shell_argument(&reference, "unused".as_ref(), d.path())
                .unwrap_or_else(|e| panic!("reference {script}: {e}"));
            for arg in ["", "a b'\";$(touch INJECTED)`touch INJECTED`\\路径\nline"] {
                assert_eq!(
                    shell_argument(&script, arg.as_ref(), d.path()).unwrap(),
                    expected.replace("REX_EXPECTED_PATH", arg),
                    "{script}",
                );
                assert!(!d.path().join("INJECTED").exists(), "{script}");
                assert!(!d.path().join("UNSELECTED").exists(), "{script}");
            }
        }
    }

    #[test]
    fn placeholder_parameter_pattern_quotes_keep_their_own_scope() {
        let d = tempfile::tempdir().unwrap();
        let arg = "a b'\";$(touch INJECTED)`touch INJECTED`\\路径*?";
        for script in [
            r#"v="$(printf '%s' "$1")tail"; printf '%s' "${v#'{}'}""#,
            r#"v="$(printf '%s' "$1")tail"; printf '%s' "${v##'{}'}""#,
            r#"v="tail$(printf '%s' "$1")"; printf '%s' "${v%'{}'}""#,
            r#"v="tail$(printf '%s' "$1")"; printf '%s' "${v%%'{}'}""#,
            r#"v="\$(printf %s $1)tail"; printf '%s' "${v#'$(printf '%s' '{}')'}""#,
        ] {
            assert_eq!(
                shell_argument(script, arg.as_ref(), d.path()).unwrap(),
                "tail",
                "{script}: {}",
                path_script(script),
            );
            assert!(!d.path().join("INJECTED").exists());
        }
    }

    #[test]
    fn placeholder_heredoc_delimiters_use_constants_and_preserve_body_semantics() {
        let d = tempfile::tempdir().unwrap();
        for script in [
            "cat <<'{}'\nbody\n{}\n",
            "cat <<{}\nbody\n{}\n",
            "cat <<\\{}\nbody\n{}\n",
            "cat <<\"{}\"\nbody\n{}\n",
            "cat <<'pre{}post'\nbody\npre{}post\n",
            "cat <<-'{}'\n\tbody\n\t{}\n",
            "cat <<'{}'\nbody\n{}\ncat <<'{}'\nbody\n{}\n",
        ] {
            assert_eq!(
                shell_argument(script, "$(touch INJECTED)".as_ref(), d.path()).unwrap(),
                "body\n".repeat(script.matches("cat ").count()),
            );
            assert!(!d.path().join("INJECTED").exists());
        }
        for script in [
            "cat <<'{}'\n{} $literal `literal` \\\n{}\n",
            "cat <<'{}'\n{}{}\n{}\n",
            "cat <<{}\n{}{}\n{}\n",
            "cat <<'{}'\n{}{}\n{}\ncat <<'{}'\n{}{}\n{}\n",
        ] {
            let arg = "a b'\";$(touch INJECTED)`touch INJECTED`\\路径";
            let expected = if script.contains("$literal") {
                format!("{arg} $literal `literal` \\\n")
            } else {
                format!("{arg}{arg}\n").repeat(if script.matches("cat ").count() == 2 {
                    2
                } else {
                    1
                })
            };
            assert_eq!(
                shell_argument(script, arg.as_ref(), d.path())
                    .unwrap_or_else(|e| panic!("{script}: {e}")),
                expected,
                "{script}"
            );
            assert!(!d.path().join("INJECTED").exists());
        }
    }

    #[test]
    fn placeholder_multiple_heredocs_run_in_sh() {
        for script in [
            "cat <<A <<B\nfirst {}{}\nA\nsecond {}{}\nB\n",
            "cat <<A <<A\nfirst {}{}\nA\nsecond {}{}\nA\n",
            "cat 3<<A <<B\nfirst {}{}\nA\nsecond {}{}\nB\n",
            "cat <<A <<B\nfirst {}{}\nA\nsecond {}{}\nB",
            "cat <<'A' <<'B'\nfirst {}{} $literal `literal` \\\nA\nsecond {}{} $literal `literal` \\\nB\n",
            "cat <<'A' <<B\nfirst {}{}\nA\nsecond {}{}\nB\n",
            "cat <<A <<\\B\nfirst {}{}\nA\nsecond {}{} $literal\nB\n",
            "cat <<\"A\" <<'B'\nfirst {}{}\nA\nsecond {}{}\nB\n",
            "cat <<-A <<-'B'\n\tfirst {}{}\n\tA\n\tsecond {}{}\n\tB\n",
            "cat <<A <<B <<C\nfirst {}\nA\nsecond {}\nB\nthird {}\nC\n",
            "cat <<'{}' <<pre{}post\nfirst {}{}\n{}\nsecond {}{}\npre{}post\n",
            "cat <<A <<B\nfirst {}\nA\nsecond {}\nB\ncat <<C\nthird {}\nC\n",
            "printf '%s' \"$(cat <<A <<B\nfirst {}{}\nA\nsecond {}{}\nB\n)\"",
            "printf '%s' \"$(cat <<'A' <<'B'\nfirst {}{}\nA\nsecond {}{}\nB\n)\"",
            "printf '%s' \"${rexue_missing:-$(cat <<A <<B\nfirst {}\nA\nsecond {}\nB\n)}\"",
            "cat <<OUTER\n$(cat <<A <<'B'\nfirst {}\nA\nsecond {}{}\nB\n)\nOUTER\n",
        ] {
            let d = tempfile::tempdir().unwrap();
            let reference = script.replace("{}", "REX_EXPECTED_PATH");
            let expected = shell_argument(&reference, "unused".as_ref(), d.path())
                .unwrap_or_else(|e| panic!("reference {script}: {e}"));
            for arg in ["", "a b'\";$(touch INJECTED)`touch INJECTED`\\路径\nline"] {
                assert_eq!(
                    shell_argument(script, arg.as_ref(), d.path())
                        .unwrap_or_else(|e| panic!("{script}: {e}")),
                    expected.replace("REX_EXPECTED_PATH", arg),
                    "{script}",
                );
                assert!(!d.path().join("INJECTED").exists(), "{script}");
            }
        }
    }

    #[test]
    fn placeholder_multiple_heredocs_preserve_overridden_expansions_in_sh() {
        let d = tempfile::tempdir().unwrap();
        let arg = "a b'\";$(touch INJECTED)`touch INJECTED`\\路径\nline";
        for script in [
            "cat <<A <<B\n$(printf '%s' '{}{}' > first; printf first >> trace)\nA\n$(printf second >> trace){}{}\nB\n",
            "cat <<-A <<-B\n\t$(printf '%s' '{}{}' > first; printf first >> trace)\n\tA\n\t$(printf second >> trace){}{}\n\tB\n",
            "printf '%s' \"$(cat <<A <<B\n$(printf '%s' '{}{}' > first; printf first >> trace)\nA\n$(printf second >> trace){}{}\nB\n)\"",
        ] {
            assert_eq!(
                shell_argument(script, arg.as_ref(), d.path())
                    .unwrap_or_else(|e| panic!("{script}: {e}")),
                format!(
                    "{arg}{arg}{}",
                    if script.starts_with("cat") { "\n" } else { "" }
                ),
                "{script}",
            );
            assert_eq!(
                std::fs::read_to_string(d.path().join("first")).unwrap(),
                format!("{arg}{arg}"),
                "{script}: {}",
                path_script(script)
            );
            assert_eq!(
                std::fs::read_to_string(d.path().join("trace")).unwrap(),
                "firstsecond"
            );
            std::fs::remove_file(d.path().join("trace")).unwrap();
            assert!(!d.path().join("INJECTED").exists());
        }
        // Quoting an overridden delimiter suppresses that body's substitutions.
        let script = "cat <<'A' <<B\n$(touch QUOTED; printf '%s' '{}{}')\nA\n{}\nB\n";
        assert_eq!(
            shell_argument(script, arg.as_ref(), d.path()).unwrap(),
            format!("{arg}\n")
        );
        assert!(!d.path().join("QUOTED").exists());
        assert!(!d.path().join("INJECTED").exists());
        let script = "cat <<A <<'B'\n$(printf '%s' '{}' > first; printf first >> trace)\nA\n$(touch QUOTED) {}{}\nB\n";
        assert_eq!(
            shell_argument(script, arg.as_ref(), d.path()).unwrap(),
            format!("$(touch QUOTED) {arg}{arg}\n")
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join("first")).unwrap(),
            arg
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join("trace")).unwrap(),
            "first"
        );
        assert!(!d.path().join("QUOTED").exists());
        assert!(!d.path().join("INJECTED").exists());
    }

    #[test]
    fn unsupported_placeholder_syntax_fails_explicitly_without_running_script() {
        let d = tempfile::tempdir().unwrap();
        for script in [
            "touch INJECTED; printf '%s' \"unterminated {}",
            "touch INJECTED; printf '%s' $(( {} + 1 ))",
            r#"touch INJECTED; printf '%s' "$(( ${rexue_missing:-{}} + 1 ))""#,
            "touch INJECTED; printf '%s' ${rexue_{}:-missing}",
            "touch INJECTED; cat <<A <<B\n$(( {} + 1 ))\nA\n{}\nB\n",
            "touch INJECTED; cat <<A <<'B'\n$(printf '%s' ${rexue_{}:-missing})\nA\n{}\nB\n",
        ] {
            let err = shell_argument(script, "$(touch INJECTED)".as_ref(), d.path())
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("exit 2") && err.contains("unsupported shell syntax"),
                "{err}"
            );
            assert!(!d.path().join("INJECTED").exists());
        }
        // Templates without a placeholder still go verbatim to the user's sh.
        assert_eq!(
            path_script("printf '%s' '$((not parsed'"),
            "printf '%s' '$((not parsed'"
        );
    }

    #[test]
    fn placeholder_binding_survives_functions_and_positional_reassignment() {
        let d = tempfile::tempdir().unwrap();
        let arg = "a b'\";$(touch INJECTED)";
        for script in [
            r#"f() { printf '%s' '{}'; }; f other"#,
            r#"set -- other; printf '%s' '{}'"#,
            r#"f() { set -- changed; printf '%s' "$(printf '%s' '{}')"; }; f other"#,
        ] {
            assert_eq!(
                shell_argument(script, arg.as_ref(), d.path()).unwrap(),
                arg,
                "{script}"
            );
            assert!(!d.path().join("INJECTED").exists());
        }
        assert_eq!(
            shell_argument(
                r#"set -- other; printf '%s\000' '{}' "$1" "${1}""#,
                arg.as_ref(),
                d.path()
            )
            .unwrap(),
            format!("{arg}\0other\0other\0")
        );
        assert_eq!(shell_argument(r#"__REXUE_PATH_VALUE=original; set -- other; printf '%s\000' "$__REXUE_PATH_VALUE" '{}'"#,arg.as_ref(),d.path()).unwrap(),format!("original\0{arg}\0"));
    }

    #[test]
    fn exchange_replies_on_the_same_child_and_aborts_without_committing() {
        let (exchange, prelude) = Exchange::start(
            Command::new("sh").args([
                "-c",
                "printf 'ready\\n'; IFS= read -r reply; printf '%s' \"$reply\"",
            ]),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(prelude, b"ready\n");
        assert_eq!(
            exchange.finish(b"validated".to_vec()).unwrap().stdout,
            b"validated"
        );

        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("committed");
        let (exchange, _) = Exchange::start(
            Command::new("sh")
                .args([
                    "-c",
                    "printf 'ready\\n'; IFS= read -r reply && touch \"$1\"",
                    "exchange",
                ])
                .arg(&marker),
            Duration::from_secs(1),
        )
        .unwrap();
        drop(exchange);
        assert!(!marker.exists());
    }

    #[test]
    fn exchange_deadline_and_early_exit_keep_errors_and_reap_children() {
        let started = Instant::now();
        let (exchange, _) = Exchange::start(
            Command::new("sh").args(["-c", "printf 'ready\\n'; sleep 3"]),
            Duration::from_millis(100),
        )
        .unwrap();
        assert!(
            exchange
                .finish(b"commit".to_vec())
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        let error = Exchange::start(
            Command::new("sh").args(["-c", "printf broken >&2; exit 4"]),
            Duration::from_secs(1),
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("exit 4") && error.to_string().contains("broken"));
    }

    #[test]
    fn failed_command_has_status_and_diagnostic() {
        let err = output(
            Command::new("sh").args(["-c", "printf broken >&2;exit 4"]),
            Duration::from_secs(1),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("exit 4") && err.contains("broken"));
    }
    #[test]
    fn descendant_cannot_keep_output_open_after_deadline() {
        let start = Instant::now();
        let e = output(
            Command::new("sh").args(["-c", "sleep 3 & exit 0"]),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(e.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
