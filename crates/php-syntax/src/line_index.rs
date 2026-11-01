/// Offset <-> (line, UTF-16 column) conversion.
#[derive(Debug, Clone)]
pub struct LineIndex {
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        LineIndex { line_starts }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts.get(line).copied().unwrap_or(*self.line_starts.last().unwrap()) as usize
    }

    pub fn position(&self, text: &str, offset: usize) -> (u32, u32) {
        let offset = offset.min(text.len());
        let line = match self.line_starts.binary_search(&(offset as u32)) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let start = self.line_starts[line] as usize;
        let mut off = offset;
        while off > start && !text.is_char_boundary(off) {
            off -= 1;
        }
        let seg = &text[start..off];
        let col = if seg.is_ascii() { seg.len() } else { seg.encode_utf16().count() };
        (line as u32, col as u32)
    }

    pub fn offset(&self, text: &str, line: u32, col: u32) -> usize {
        let line = line as usize;
        if line >= self.line_starts.len() {
            return text.len();
        }
        let start = self.line_starts[line] as usize;
        let end = self.line_starts.get(line + 1).map(|e| *e as usize).unwrap_or(text.len());
        let seg = &text[start..end];
        if seg.is_ascii() {
            return (start + col as usize).min(end);
        }
        let mut units = 0u32;
        for (i, ch) in seg.char_indices() {
            if units >= col || ch == '\n' {
                return start + i;
            }
            units += ch.len_utf16() as u32;
        }
        end
    }
}
