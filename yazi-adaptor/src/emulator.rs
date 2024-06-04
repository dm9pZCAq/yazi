use std::{env, io::{stderr, LineWriter}};

use anyhow::{anyhow, Result};
use crossterm::{cursor::{RestorePosition, SavePosition}, execute, style::Print, terminal::{disable_raw_mode, enable_raw_mode}};
use scopeguard::defer;
use tracing::warn;
use yazi_shared::{env_exists, term::Term};

use crate::{Adaptor, CLOSE, ESCAPE, START, TMUX};

#[derive(Clone, Debug)]
pub enum Emulator {
	Unknown(Vec<Adaptor>),
	Kitty,
	Konsole,
	Iterm2,
	WezTerm,
	Foot,
	Ghostty,
	BlackBox,
	VSCode,
	Tabby,
	Hyper,
	Mintty,
	Neovim,
	Apple,
	Urxvt,
}

impl Emulator {
	pub fn adapters(self) -> Vec<Adaptor> {
		match self {
			Self::Unknown(adapters) => adapters,
			Self::Kitty => vec![Adaptor::Kitty],
			Self::Konsole => vec![Adaptor::KittyOld],
			Self::Iterm2 => vec![Adaptor::Iterm2, Adaptor::Sixel],
			Self::WezTerm => vec![Adaptor::Iterm2, Adaptor::Sixel],
			Self::Foot => vec![Adaptor::Sixel],
			Self::Ghostty => vec![Adaptor::KittyOld],
			Self::BlackBox => vec![Adaptor::Sixel],
			Self::VSCode => vec![Adaptor::Iterm2, Adaptor::Sixel],
			Self::Tabby => vec![Adaptor::Iterm2, Adaptor::Sixel],
			Self::Hyper => vec![Adaptor::Iterm2, Adaptor::Sixel],
			Self::Mintty => vec![Adaptor::Iterm2],
			Self::Neovim => vec![],
			Self::Apple => vec![],
			Self::Urxvt => vec![],
		}
	}
}

impl Emulator {
	pub fn detect() -> Self {
		if env_exists("NVIM_LOG_FILE") && env_exists("NVIM") {
			return Self::Neovim;
		}

		let vars = [
			("KITTY_WINDOW_ID", Self::Kitty),
			("KONSOLE_VERSION", Self::Konsole),
			("ITERM_SESSION_ID", Self::Iterm2),
			("WEZTERM_EXECUTABLE", Self::WezTerm),
			("GHOSTTY_RESOURCES_DIR", Self::Ghostty),
			("VSCODE_INJECTION", Self::VSCode),
			("TABBY_CONFIG_DIRECTORY", Self::Tabby),
		];
		match vars.into_iter().find(|v| env_exists(v.0)) {
			Some(var) => return var.1,
			None => warn!("[Adaptor] No special environment variables detected"),
		}

		let (term, program) = Self::via_env();
		match program.as_str() {
			"iTerm.app" => return Self::Iterm2,
			"WezTerm" => return Self::WezTerm,
			"ghostty" => return Self::Ghostty,
			"BlackBox" => return Self::BlackBox,
			"vscode" => return Self::VSCode,
			"Tabby" => return Self::Tabby,
			"Hyper" => return Self::Hyper,
			"mintty" => return Self::Mintty,
			"Apple_Terminal" => return Self::Apple,
			_ => warn!("[Adaptor] Unknown TERM_PROGRAM: {program}"),
		}
		match term.as_str() {
			"xterm-kitty" => return Self::Kitty,
			"foot" => return Self::Foot,
			"foot-extra" => return Self::Foot,
			"xterm-ghostty" => return Self::Ghostty,
			"rxvt-unicode-256color" => return Self::Urxvt,
			_ => warn!("[Adaptor] Unknown TERM: {term}"),
		}

		Self::via_csi().unwrap_or(Self::Unknown(vec![]))
	}

	pub fn via_env() -> (String, String) {
		fn tmux_env(name: &str) -> Result<String> {
			let output = std::process::Command::new("tmux").args(["show-environment", name]).output()?;

			String::from_utf8(output.stdout)?
				.trim()
				.strip_prefix(&format!("{name}="))
				.map_or_else(|| Err(anyhow!("")), |s| Ok(s.to_string()))
		}

		let mut term = env::var("TERM").unwrap_or_default();
		let mut program = env::var("TERM_PROGRAM").unwrap_or_default();

		if *TMUX {
			term = tmux_env("TERM").unwrap_or(term);
			program = tmux_env("TERM_PROGRAM").unwrap_or(program);
		}

		(term, program)
	}

	pub fn via_csi() -> Result<Self> {
		defer! { disable_raw_mode().ok(); }
		enable_raw_mode()?;

		execute!(
			LineWriter::new(stderr()),
			SavePosition,
			Print(format!(
				"{}[>q{}_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA{}\\{}[c{}",
				START, ESCAPE, ESCAPE, ESCAPE, CLOSE
			)),
			RestorePosition
		)?;

		let resp = futures::executor::block_on(Term::read_until_da1())?;
		let names = [
			("kitty", Self::Kitty),
			("Konsole", Self::Konsole),
			("iTerm2", Self::Iterm2),
			("WezTerm", Self::WezTerm),
			("foot", Self::Foot),
			("ghostty", Self::Ghostty),
		];

		for (name, emulator) in names.iter() {
			if resp.contains(name) {
				return Ok(emulator.clone());
			}
		}

		let mut adapters = Vec::with_capacity(2);
		if resp.contains("\x1b_Gi=31;OK") {
			adapters.push(Adaptor::KittyOld);
		}
		if ["?4;", "?4c", ";4;", ";4c"].iter().any(|s| resp.contains(s)) {
			adapters.push(Adaptor::Sixel);
		}

		Ok(Self::Unknown(adapters))
	}

	pub fn move_lock<F, T>((x, y): (u16, u16), cb: F) -> Result<T>
	where
		F: FnOnce(&mut std::io::BufWriter<std::io::StderrLock>) -> Result<T>,
	{
		use std::{io::Write, thread, time::Duration};

		use crossterm::{cursor::{Hide, MoveTo, RestorePosition, SavePosition, Show}, queue};

		let mut buf = std::io::BufWriter::new(stderr().lock());

		// I really don't want to add this,
		// But tmux and ConPTY sometimes cause the cursor position to get out of sync.
		if *TMUX || cfg!(windows) {
			execute!(buf, SavePosition, MoveTo(x, y), Show)?;
			execute!(buf, MoveTo(x, y), Show)?;
			execute!(buf, MoveTo(x, y), Show)?;
			thread::sleep(Duration::from_millis(1));
		} else {
			queue!(buf, SavePosition, MoveTo(x, y))?;
		}

		let result = cb(&mut buf);
		if *TMUX || cfg!(windows) {
			queue!(buf, Hide, RestorePosition)?;
		} else {
			queue!(buf, RestorePosition)?;
		}

		buf.flush()?;
		result
	}
}
