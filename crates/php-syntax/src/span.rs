#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start: start as u32, end: end as u32 }
    }

    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Inclusive of the end position, so a cursor right after an identifier still hits it.
    pub fn contains(&self, offset: usize) -> bool {
        (self.start as usize) <= offset && offset <= (self.end as usize)
    }

    pub fn to(self, other: Span) -> Span {
        Span { start: self.start.min(other.start), end: self.end.max(other.end) }
    }

    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        src.get(self.start as usize..self.end as usize).unwrap_or("")
    }
}
