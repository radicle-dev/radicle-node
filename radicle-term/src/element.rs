use std::fmt;
use std::io::IsTerminal;
use std::ops::Deref;
use std::{io, vec};

use crate::cell::Cell;
use crate::{viewport, Color, Filled, Label, Style};

/// Rendering constraint.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Constraint {
    /// Minimum space the element can take.
    pub min: Size,
    /// Maximum space the element can take.
    pub max: Size,
}

impl Default for Constraint {
    fn default() -> Self {
        Self::UNBOUNDED
    }
}

impl Constraint {
    /// Can satisfy any size of object.
    pub const UNBOUNDED: Self = Self {
        min: Size::MIN,
        max: Size::MAX,
    };

    /// Create a new constraint.
    pub fn new(min: Size, max: Size) -> Self {
        assert!(min.cols <= max.cols && min.rows <= max.rows);

        Self { min, max }
    }

    /// A constraint with only a maximum size.
    pub fn max(max: Size) -> Self {
        Self {
            min: Size::MIN,
            max,
        }
    }

    /// A constraint that can only be satisfied by a single column size.
    /// The rows are unconstrained.
    pub fn tight(cols: usize) -> Self {
        Self {
            min: Size::new(cols, 1),
            max: Size::new(cols, usize::MAX),
        }
    }

    /// Create a constraint from the terminal environment.
    /// Returns [`None`] if the output device is not a terminal.
    pub fn from_env() -> Option<Self> {
        if io::stdout().is_terminal() {
            Some(Self::max(viewport().unwrap_or(Size::MAX)))
        } else {
            None
        }
    }
}

/// A text element that has a size and can be rendered to the terminal.
pub trait Element: fmt::Debug {
    /// Get the size of the element, in rows and columns.
    fn size(&self, parent: Constraint) -> Size;

    #[must_use]
    /// Render the element as lines of text that can be printed.
    fn render(&self, parent: Constraint) -> Vec<Line>;

    /// Get the number of columns occupied by this element.
    fn columns(&self, parent: Constraint) -> usize {
        self.size(parent).cols
    }

    /// Get the number of rows occupied by this element.
    fn rows(&self, parent: Constraint) -> usize {
        self.size(parent).rows
    }

    /// Print this element to stdout.
    fn print(&self) {
        for line in self.render(Constraint::from_env().unwrap_or_default()) {
            println!("{}", line.to_string().trim_end());
        }
    }

    /// Write using the given constraints to `stdout`.
    fn write(&self, constraints: Constraint) -> io::Result<()>
    where
        Self: Sized,
    {
        self::write_to(self, &mut io::stdout(), constraints)
    }

    #[must_use]
    /// Return a string representation of this element.
    fn display(&self, constraints: Constraint) -> String {
        let mut out = String::new();
        for line in self.render(constraints) {
            out.extend(line.into_iter().map(|l| l.to_string()));
            out.push('\n');
        }
        out
    }
}

impl<'a> Element for Box<dyn Element + 'a> {
    fn size(&self, parent: Constraint) -> Size {
        self.deref().size(parent)
    }

    fn render(&self, parent: Constraint) -> Vec<Line> {
        self.deref().render(parent)
    }

    fn print(&self) {
        self.deref().print()
    }
}

impl<T: Element> Element for &T {
    fn size(&self, parent: Constraint) -> Size {
        (*self).size(parent)
    }

    fn render(&self, parent: Constraint) -> Vec<Line> {
        (*self).render(parent)
    }

    fn print(&self) {
        (*self).print()
    }
}

/// Write using the given constraints, to a writer.
pub fn write_to(
    elem: &impl Element,
    writer: &mut impl io::Write,
    constraints: Constraint,
) -> io::Result<()> {
    for line in elem.render(constraints) {
        writeln!(writer, "{}", line.to_string().trim_end())?;
    }
    Ok(())
}

/// A line of text that has styling and can be displayed.
#[derive(Clone, Default, Debug)]
pub struct Line {
    items: Vec<Label>,
}

impl Line {
    /// Create a new line.
    pub fn new(item: impl Into<Label>) -> Self {
        Self {
            items: vec![item.into()],
        }
    }

    /// Create a blank line.
    pub fn blank() -> Self {
        Self { items: vec![] }
    }

    /// Return a styled line by styling all its labels.
    pub fn style(self, style: Style) -> Line {
        Self {
            items: self
                .items
                .into_iter()
                .map(|l| {
                    let style = l.paint().style().merge(style);
                    l.style(style)
                })
                .collect(),
        }
    }

    /// Return a line with a single space between the given labels.
    pub fn spaced(items: impl IntoIterator<Item = Label>) -> Self {
        let mut line = Self::default();
        for item in items.into_iter() {
            line.push(item);
            line.push(Label::space());
        }
        line.items.pop();
        line
    }

    /// Add an item to this line.
    pub fn item(mut self, item: impl Into<Label>) -> Self {
        self.push(item);
        self
    }

    /// Add multiple items to this line.
    pub fn extend(mut self, items: impl IntoIterator<Item = Label>) -> Self {
        self.items.extend(items);
        self
    }

    /// Add an item to this line.
    pub fn push(&mut self, item: impl Into<Label>) {
        self.items.push(item.into());
    }

    /// Pad this line to occupy the given width.
    pub fn pad(&mut self, width: usize) {
        let w = self.width();

        if width > w {
            let pad = width - w;
            let bg = if let Some(last) = self.items.last() {
                last.background()
            } else {
                Color::Unset
            };
            self.items.push(Label::new(" ".repeat(pad).as_str()).bg(bg));
        }
    }

    /// Truncate this line to the given width.
    pub fn truncate(&mut self, width: usize, delim: &str) {
        while self.width() > width {
            let total = self.width();

            if total - self.items.last().map_or(0, Cell::width) > width {
                self.items.pop();
            } else if let Some(item) = self.items.last_mut() {
                *item = item.truncate(width - (total - Cell::width(item)), delim);
            }
        }
    }

    /// Get the actual column width of this line.
    pub fn width(&self) -> usize {
        self.items.iter().map(Cell::width).sum()
    }

    /// Create a line that contains a single space.
    pub fn space(mut self) -> Self {
        self.items.push(Label::space());
        self
    }

    /// Box this line as an [`Element`].
    pub fn boxed(self) -> Box<dyn Element> {
        Box::new(self)
    }

    /// Return a filled line.
    pub fn filled(self, color: Color) -> Filled<Self> {
        Filled { item: self, color }
    }
}

impl IntoIterator for Line {
    type Item = Label;
    type IntoIter = Box<dyn Iterator<Item = Label>>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.items.into_iter())
    }
}

impl<T: Into<Label>> From<T> for Line {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl From<Vec<Label>> for Line {
    fn from(items: Vec<Label>) -> Self {
        Self { items }
    }
}

impl Element for Line {
    fn size(&self, _parent: Constraint) -> Size {
        Size::new(self.items.iter().map(Cell::width).sum(), 1)
    }

    fn render(&self, _parent: Constraint) -> Vec<Line> {
        vec![self.clone()]
    }
}

impl Element for Vec<Line> {
    fn size(&self, parent: Constraint) -> Size {
        let width = self
            .iter()
            .map(|e| e.columns(parent))
            .max()
            .unwrap_or_default();
        let height = self.len();

        Size::new(width, height)
    }

    fn render(&self, parent: Constraint) -> Vec<Line> {
        self.iter()
            .cloned()
            .flat_map(|l| l.render(parent))
            .collect()
    }
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for item in &self.items {
            write!(f, "{item}")?;
        }
        Ok(())
    }
}

/// Size of a text element, in columns and rows.
#[derive(Clone, Debug, Default, Copy, PartialEq, Eq)]
pub struct Size {
    /// Columns occupied.
    pub cols: usize,
    /// Rows occupied.
    pub rows: usize,
}

impl Size {
    /// Minimum size.
    pub const MIN: Self = Self {
        cols: usize::MIN,
        rows: usize::MIN,
    };
    /// Maximum size.
    pub const MAX: Self = Self {
        cols: usize::MAX,
        rows: usize::MAX,
    };

    /// Create a new [`Size`].
    pub fn new(cols: usize, rows: usize) -> Self {
        Self { cols, rows }
    }

    /// Constrain size.
    pub fn constrain(self, c: Constraint) -> Self {
        Self {
            cols: self.cols.clamp(c.min.cols, c.max.cols),
            rows: self.rows.clamp(c.min.rows, c.max.rows),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_truncate() {
        let line = Line::default().item("banana").item("peach").item("apple");

        let mut actual = line.clone();
        actual = actual.truncate(9, "…");
        assert_eq!(actual.to_string(), "bananape…");

        let mut actual = line.clone();
        actual = actual.truncate(7, "…");
        assert_eq!(actual.to_string(), "banana…");

        let mut actual = line.clone();
        actual = actual.truncate(1, "…");
        assert_eq!(actual.to_string(), "…");

        let mut actual = line;
        actual = actual.truncate(0, "…");
        assert_eq!(actual.to_string(), "");
    }

    #[test]
    fn test_width() {
        // Nb. This might not display correctly in some editors or terminals.
        let line = Line::new("Radicle Heartwood Protocol & Stack ❤️🪵");
        assert_eq!(line.width(), 39, "{line}");
        let line = Line::new("❤\u{fe0f}");
        assert_eq!(line.width(), 2, "{line}");
        let line = Line::new("❤️");
        assert_eq!(line.width(), 2, "{line}");
    }
}
