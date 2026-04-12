//! Cell-based viewport — a rectangular region in character coordinates.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellViewport {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

impl CellViewport {
    pub const fn new(col: u16, row: u16, cols: u16, rows: u16) -> Self {
        Self { col, row, cols, rows }
    }

    pub const fn right(&self) -> u16 {
        self.col + self.cols
    }

    pub const fn bottom(&self) -> u16 {
        self.row + self.rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds() {
        let vp = CellViewport::new(5, 10, 80, 24);
        assert_eq!(vp.right(), 85);
        assert_eq!(vp.bottom(), 34);
    }
}
