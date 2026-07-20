//! Small, terminal-aware presentation helpers for human-facing CLI output.

use std::fmt::{self, Display};

use anstyle::{AnsiColor, Style};

const SUCCESS: Style = AnsiColor::Green.on_default().bold();
const ERROR: Style = AnsiColor::Red.on_default().bold();
const WARNING: Style = AnsiColor::Yellow.on_default().bold();
const HEADING: Style = AnsiColor::Cyan.on_default().bold();
const ACCENT: Style = AnsiColor::BrightCyan.on_default().bold();
const COMMAND: Style = AnsiColor::BrightYellow.on_default();
const MUTED: Style = AnsiColor::BrightBlack.on_default();
const ADDED: Style = AnsiColor::Green.on_default();
const REMOVED: Style = AnsiColor::Red.on_default();
const MODIFIED: Style = AnsiColor::Yellow.on_default();

pub struct Styled<T> {
    value: T,
    style: Style,
}

impl<T: Display> Display for Styled<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}{value}{}",
            self.style,
            self.style.render_reset(),
            value = self.value
        )
    }
}

fn styled<T: Display>(value: T, style: Style) -> Styled<T> {
    Styled { value, style }
}

pub fn checkpoint<T: Display>(value: T) -> Styled<T> {
    styled(value, ACCENT)
}

pub fn command<T: Display>(value: T) -> Styled<T> {
    styled(value, COMMAND)
}

pub fn muted<T: Display>(value: T) -> Styled<T> {
    styled(value, MUTED)
}

pub fn added<T: Display>(value: T) -> Styled<T> {
    styled(value, ADDED)
}

pub fn removed<T: Display>(value: T) -> Styled<T> {
    styled(value, REMOVED)
}

pub fn modified<T: Display>(value: T) -> Styled<T> {
    styled(value, MODIFIED)
}

pub fn line(message: fmt::Arguments<'_>) {
    anstream::println!("{message}");
}

pub fn success(message: fmt::Arguments<'_>) {
    anstream::println!("{SUCCESS}✓{SUCCESS:#} {message}");
}

pub fn warning(message: fmt::Arguments<'_>) {
    anstream::eprintln!("{WARNING}Warning:{WARNING:#} {message}");
}

pub fn error(message: fmt::Arguments<'_>) {
    anstream::eprintln!("{ERROR}Error:{ERROR:#} {message}");
}

pub fn heading(title: &str) {
    anstream::println!("{HEADING}{title}:{HEADING:#}");
}

pub fn field(label: &str, value: fmt::Arguments<'_>) {
    anstream::println!("{HEADING}{label}:{HEADING:#} {value}");
}

pub fn ok(label: &str, detail: fmt::Arguments<'_>) {
    anstream::println!("{SUCCESS}✓{SUCCESS:#} {label}: {detail}");
}

pub fn notice(label: &str, detail: fmt::Arguments<'_>) {
    anstream::println!("{WARNING}!{WARNING:#} {label}: {detail}");
}

pub fn failed(label: &str, detail: fmt::Arguments<'_>) {
    anstream::println!("{ERROR}✗{ERROR:#} {label}: {detail}");
}
