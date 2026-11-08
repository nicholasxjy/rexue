//! Rex's bundled program artwork, rendered through Kitty Unicode placeholders.
use std::{
    io::{IsTerminal, Write},
    path::Path,
};

const ASSETS: &[(&str, &[u8])] = &[
    ("amp", include_bytes!("../assets/programs/amp.png")),
    ("android", include_bytes!("../assets/programs/android.png")),
    (
        "antigravity",
        include_bytes!("../assets/programs/antigravity.png"),
    ),
    (
        "apple-container",
        include_bytes!("../assets/programs/apple-container.png"),
    ),
    ("azure", include_bytes!("../assets/programs/azure.png")),
    ("bash", include_bytes!("../assets/programs/bash.png")),
    ("bat", include_bytes!("../assets/programs/bat.png")),
    ("bazel", include_bytes!("../assets/programs/bazel.png")),
    (
        "boundary",
        include_bytes!("../assets/programs/boundary.png"),
    ),
    ("brew", include_bytes!("../assets/programs/brew.png")),
    ("browser", include_bytes!("../assets/programs/browser.png")),
    ("btop", include_bytes!("../assets/programs/btop.png")),
    ("buck", include_bytes!("../assets/programs/buck.png")),
    (
        "buildkite",
        include_bytes!("../assets/programs/buildkite.png"),
    ),
    ("bun", include_bytes!("../assets/programs/bun.png")),
    ("butler", include_bytes!("../assets/programs/butler.png")),
    ("caddy", include_bytes!("../assets/programs/caddy.png")),
    (
        "caffeinate",
        include_bytes!("../assets/programs/caffeinate.png"),
    ),
    ("charm", include_bytes!("../assets/programs/charm.png")),
    ("clang", include_bytes!("../assets/programs/clang.png")),
    ("claude", include_bytes!("../assets/programs/claude.png")),
    (
        "cloudflare",
        include_bytes!("../assets/programs/cloudflare.png"),
    ),
    ("coder", include_bytes!("../assets/programs/coder.png")),
    ("codex", include_bytes!("../assets/programs/codex.png")),
    ("consul", include_bytes!("../assets/programs/consul.png")),
    ("copilot", include_bytes!("../assets/programs/copilot.png")),
    ("crush", include_bytes!("../assets/programs/crush.png")),
    ("cua", include_bytes!("../assets/programs/cua.png")),
    ("cursor", include_bytes!("../assets/programs/cursor.png")),
    ("d2", include_bytes!("../assets/programs/d2.png")),
    ("dart", include_bytes!("../assets/programs/dart.png")),
    ("datadog", include_bytes!("../assets/programs/datadog.png")),
    ("dbcli", include_bytes!("../assets/programs/dbcli.png")),
    ("delta", include_bytes!("../assets/programs/delta.png")),
    ("depot", include_bytes!("../assets/programs/depot.png")),
    ("devin", include_bytes!("../assets/programs/devin.png")),
    ("docker", include_bytes!("../assets/programs/docker.png")),
    ("dotnet", include_bytes!("../assets/programs/dotnet.png")),
    ("droid", include_bytes!("../assets/programs/droid.png")),
    ("duckdb", include_bytes!("../assets/programs/duckdb.png")),
    ("e2b", include_bytes!("../assets/programs/e2b.png")),
    ("elixir", include_bytes!("../assets/programs/elixir.png")),
    ("emacs", include_bytes!("../assets/programs/emacs.png")),
    ("expo", include_bytes!("../assets/programs/expo.png")),
    ("fish", include_bytes!("../assets/programs/fish.png")),
    ("flutter", include_bytes!("../assets/programs/flutter.png")),
    ("fm", include_bytes!("../assets/programs/fm.png")),
    ("fx", include_bytes!("../assets/programs/fx.png")),
    ("gemini", include_bytes!("../assets/programs/gemini.png")),
    ("git", include_bytes!("../assets/programs/git.png")),
    (
        "gitchanges",
        include_bytes!("../assets/programs/gitchanges.png"),
    ),
    ("github", include_bytes!("../assets/programs/github.png")),
    ("gitlab", include_bytes!("../assets/programs/gitlab.png")),
    ("gnu", include_bytes!("../assets/programs/gnu.png")),
    ("go", include_bytes!("../assets/programs/go.png")),
    ("goose", include_bytes!("../assets/programs/goose.png")),
    ("gradle", include_bytes!("../assets/programs/gradle.png")),
    ("grok", include_bytes!("../assets/programs/grok.png")),
    ("hermes", include_bytes!("../assets/programs/hermes.png")),
    ("heroku", include_bytes!("../assets/programs/heroku.png")),
    ("homey", include_bytes!("../assets/programs/homey.png")),
    ("htop", include_bytes!("../assets/programs/htop.png")),
    (
        "hugging-face",
        include_bytes!("../assets/programs/hugging-face.png"),
    ),
    ("hunk", include_bytes!("../assets/programs/hunk.png")),
    ("hx", include_bytes!("../assets/programs/hx.png")),
    ("ibm-bob", include_bytes!("../assets/programs/ibm-bob.png")),
    (
        "inspector",
        include_bytes!("../assets/programs/inspector.png"),
    ),
    ("jj", include_bytes!("../assets/programs/jj.png")),
    ("julia", include_bytes!("../assets/programs/julia.png")),
    ("junie", include_bytes!("../assets/programs/junie.png")),
    ("jupyter", include_bytes!("../assets/programs/jupyter.png")),
    ("k9s", include_bytes!("../assets/programs/k9s.png")),
    ("kakoune", include_bytes!("../assets/programs/kakoune.png")),
    ("kiro", include_bytes!("../assets/programs/kiro.png")),
    ("kube", include_bytes!("../assets/programs/kube.png")),
    (
        "llama-cpp",
        include_bytes!("../assets/programs/llama-cpp.png"),
    ),
    ("log", include_bytes!("../assets/programs/log.png")),
    ("maki", include_bytes!("../assets/programs/maki.png")),
    ("mastra", include_bytes!("../assets/programs/mastra.png")),
    ("meta", include_bytes!("../assets/programs/meta.png")),
    ("micro", include_bytes!("../assets/programs/micro.png")),
    ("minimal", include_bytes!("../assets/programs/minimal.png")),
    ("mise", include_bytes!("../assets/programs/mise.png")),
    ("mistral", include_bytes!("../assets/programs/mistral.png")),
    ("modal", include_bytes!("../assets/programs/modal.png")),
    ("mole", include_bytes!("../assets/programs/mole.png")),
    ("moon", include_bytes!("../assets/programs/moon.png")),
    ("nano", include_bytes!("../assets/programs/nano.png")),
    ("neovim", include_bytes!("../assets/programs/neovim.png")),
    (
        "nextflow",
        include_bytes!("../assets/programs/nextflow.png"),
    ),
    ("nix", include_bytes!("../assets/programs/nix.png")),
    ("node", include_bytes!("../assets/programs/node.png")),
    ("nomad", include_bytes!("../assets/programs/nomad.png")),
    ("nushell", include_bytes!("../assets/programs/nushell.png")),
    ("ollama", include_bytes!("../assets/programs/ollama.png")),
    ("omp", include_bytes!("../assets/programs/omp.png")),
    (
        "openclaw",
        include_bytes!("../assets/programs/openclaw.png"),
    ),
    (
        "opencode",
        include_bytes!("../assets/programs/opencode.png"),
    ),
    ("opscope", include_bytes!("../assets/programs/opscope.png")),
    (
        "orbstack",
        include_bytes!("../assets/programs/orbstack.png"),
    ),
    ("oxide", include_bytes!("../assets/programs/oxide.png")),
    ("pack", include_bytes!("../assets/programs/pack.png")),
    ("packer", include_bytes!("../assets/programs/packer.png")),
    ("php", include_bytes!("../assets/programs/php.png")),
    ("pi", include_bytes!("../assets/programs/pi.png")),
    ("pixi", include_bytes!("../assets/programs/pixi.png")),
    (
        "placeholder",
        include_bytes!("../assets/programs/placeholder.png"),
    ),
    ("pnpm", include_bytes!("../assets/programs/pnpm.png")),
    ("podman", include_bytes!("../assets/programs/podman.png")),
    (
        "poolside-icon",
        include_bytes!("../assets/programs/poolside-icon.png"),
    ),
    ("porter", include_bytes!("../assets/programs/porter.png")),
    (
        "postgres",
        include_bytes!("../assets/programs/postgres.png"),
    ),
    (
        "prime-agent",
        include_bytes!("../assets/programs/prime-agent.png"),
    ),
    ("proto", include_bytes!("../assets/programs/proto.png")),
    (
        "proton-drive",
        include_bytes!("../assets/programs/proton-drive.png"),
    ),
    (
        "proton-pass",
        include_bytes!("../assets/programs/proton-pass.png"),
    ),
    ("python", include_bytes!("../assets/programs/python.png")),
    ("railway", include_bytes!("../assets/programs/railway.png")),
    ("rclone", include_bytes!("../assets/programs/rclone.png")),
    ("redis", include_bytes!("../assets/programs/redis.png")),
    ("render", include_bytes!("../assets/programs/render.png")),
    ("rex", include_bytes!("../assets/programs/rex.png")),
    ("ruby", include_bytes!("../assets/programs/ruby.png")),
    ("rust", include_bytes!("../assets/programs/rust.png")),
    ("sail", include_bytes!("../assets/programs/sail.png")),
    ("sanity", include_bytes!("../assets/programs/sanity.png")),
    ("shell", include_bytes!("../assets/programs/shell.png")),
    (
        "shell-container",
        include_bytes!("../assets/programs/shell-container.png"),
    ),
    ("shopify", include_bytes!("../assets/programs/shopify.png")),
    ("sleep", include_bytes!("../assets/programs/sleep.png")),
    (
        "snowflake",
        include_bytes!("../assets/programs/snowflake.png"),
    ),
    ("sofka", include_bytes!("../assets/programs/sofka.png")),
    ("sqlite", include_bytes!("../assets/programs/sqlite.png")),
    ("stripe", include_bytes!("../assets/programs/stripe.png")),
    ("swift", include_bytes!("../assets/programs/swift.png")),
    (
        "tailscale",
        include_bytes!("../assets/programs/tailscale.png"),
    ),
    ("task", include_bytes!("../assets/programs/task.png")),
    (
        "terminal",
        include_bytes!("../assets/programs/terminal.png"),
    ),
    (
        "terraform",
        include_bytes!("../assets/programs/terraform.png"),
    ),
    ("top", include_bytes!("../assets/programs/top.png")),
    (
        "turborepo",
        include_bytes!("../assets/programs/turborepo.png"),
    ),
    (
        "unreal-engine",
        include_bytes!("../assets/programs/unreal-engine.png"),
    ),
    ("uv", include_bytes!("../assets/programs/uv.png")),
    ("vagrant", include_bytes!("../assets/programs/vagrant.png")),
    ("vakna", include_bytes!("../assets/programs/vakna.png")),
    ("vault", include_bytes!("../assets/programs/vault.png")),
    ("vercel", include_bytes!("../assets/programs/vercel.png")),
    ("vim", include_bytes!("../assets/programs/vim.png")),
    ("vite", include_bytes!("../assets/programs/vite.png")),
    (
        "vite-plus",
        include_bytes!("../assets/programs/vite-plus.png"),
    ),
    (
        "waypoint",
        include_bytes!("../assets/programs/waypoint.png"),
    ),
    ("xcode", include_bytes!("../assets/programs/xcode.png")),
    ("yazi", include_bytes!("../assets/programs/yazi.png")),
    ("zig", include_bytes!("../assets/programs/zig.png")),
    ("zsh", include_bytes!("../assets/programs/zsh.png")),
];

pub(crate) struct Icon {
    pub name: &'static str,
    id: u32,
    png: &'static [u8],
}

pub(crate) fn available() -> bool {
    std::io::stderr().is_terminal()
        && std::env::var("TERM_PROGRAM").is_ok_and(|value| value == "rex")
        && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
}

fn image_id(index: usize) -> u32 {
    ((std::process::id() % 65535 + 1) << 8) | index as u32
}

pub(crate) fn icon(program: &str) -> Option<Icon> {
    let name = Path::new(program)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase();
    let name = name.trim_end_matches(".exe");
    let name = match name {
        "zsh" => "shell",
        "claude-code" => "claude",
        "pi-coding-agent" => "pi",
        "gemini-cli" => "gemini",
        "cursor-agent" => "cursor",
        "nvim" => "neovim",
        "python3" => "python",
        "nu" => "nushell",
        "cargo" | "rustc" => "rust",
        value => value,
    };
    let index = ASSETS.binary_search_by_key(&name, |(name, _)| *name).ok()?;
    let (name, png) = ASSETS[index];
    Some(Icon {
        name,
        id: image_id(index),
        png,
    })
}

impl Icon {
    pub fn placeholder(&self) -> String {
        let (r, g, b) = (self.id >> 16, (self.id >> 8) & 255, self.id & 255);
        format!("\x1b[38;2;{r};{g};{b}m\u{10eeee}\u{305}\u{305}\u{10eeee}\u{305}\u{30d}\x1b[39m ")
    }

    pub fn transmit(&self, output: &mut Vec<u8>) {
        let encoded = base64(self.png);
        let mut chunks = encoded.as_bytes().chunks(4096).peekable();
        let mut first = true;
        while let Some(chunk) = chunks.next() {
            let more = u8::from(chunks.peek().is_some());
            if first {
                write!(
                    output,
                    "\x1b_Ga=T,f=100,i={},U=1,c=2,r=1,q=2,m={more};",
                    self.id
                )
                .unwrap();
                first = false;
            } else {
                write!(output, "\x1b_Gm={more};").unwrap();
            }
            output.extend_from_slice(chunk);
            output.extend_from_slice(b"\x1b\\");
        }
    }
}

pub(crate) fn clear() {
    if let Ok(mut terminal) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        for index in 0..ASSETS.len() {
            let _ = write!(terminal, "\x1b_Ga=d,d=I,i={},q=2\x1b\\", image_id(index));
        }
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        result.push(ALPHABET[(a >> 2) as usize] as char);
        result.push(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        result.push(if chunk.len() > 1 {
            ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            ALPHABET[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_assets_aliases_and_placeholder_width_are_valid() {
        assert!(ASSETS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(ASSETS.len() < 256);
        for (name, png) in ASSETS {
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "{name}");
            assert_eq!(&png[16..24], &[0, 0, 0, 48, 0, 0, 0, 48], "{name}");
            let text = strip_ansi_escapes::strip_str(icon(name).unwrap().placeholder());
            assert_eq!(unicode_width::UnicodeWidthStr::width(text.as_str()), 3);
        }
        assert_eq!(icon("/usr/bin/zsh").unwrap().name, "shell");
        assert_eq!(icon("/usr/bin/Python3.exe").unwrap().name, "python");
        assert_eq!(icon("Python").unwrap().name, "python");
        assert_eq!(icon("claude-code").unwrap().name, "claude");
        assert_eq!(icon("pi-coding-agent").unwrap().name, "pi");
        assert!(icon("unknown-program").is_none());
    }
    #[test]
    fn image_transfer_uses_valid_base64_chunks() {
        for (input, expected) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v")] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
        let mut output = Vec::new();
        icon("pi").unwrap().transmit(&mut output);
        assert!(output.starts_with(b"\x1b_Ga=T,f=100,"));
        assert!(output.ends_with(b"\x1b\\"));
        assert!(
            output.split(|byte| *byte == b';').skip(1).all(|part| part
                .split(|byte| *byte == 27)
                .next()
                .unwrap()
                .len()
                <= 4096)
        );
    }
}
