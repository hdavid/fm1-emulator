// SPDX-License-Identifier: GPL-3.0-only
// The panel LEDs, seen from the pins that drive them (fm1_input.h wiring):
// four LED lines (PA9, PA10, PH6, PH9) light the LED of matrix row PA5, PA6,
// PA7 or PA8 on each column the 74HC595 chain holds low. An LED is lit while
// its line is driven high and its column is latched; its brightness is the
// device time it was lit over the time its column was selected, so full on
// is 1 whatever the firmware's scan rate. Only pin states and device time
// are observed: guest execution never sees this.

/// Matrix columns (74HC595 outputs 0..10).
pub const COLUMNS: usize = 11;
/// LED lines, by the matrix row bit they light (1..=4: PA5..PA8).
pub const LINES: usize = 4;
/// (GPIO port, pin) of the LED line for row bit 1 + index.
pub const LINE_PINS: [(usize, u32); LINES] = [(0, 9), (0, 10), (7, 6), (7, 9)];
/// Key id at (row bit 1 + line, column), -1 = none (fm1_input.h FM1_KEYMAP).
pub const KEYMAP: [[i8; COLUMNS]; LINES] = [
    [5, 11, 4, 10, 3, 9, 2, 8, -1, -1, -1],
    [34, 35, 36, 37, 38, 40, 39, 13, 7, 6, 12],
    [23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33],
    [0, 1, 15, 14, 17, 16, 19, 18, 20, 21, 22],
];
/// Buttons 0..13 and note keys 14..40.
pub const KEYS: usize = 41;

/// Brightness per LED, `[column][line]`, 0 (dark) ..= 1 (lit whenever its
/// column was selected).
pub type Brightness = [[f32; LINES]; COLUMNS];

#[derive(Clone, Debug, Default)]
pub struct Leds {
    /// Device time (oscillator ticks), the last pin change, the window's start.
    now: u64,
    since: u64,
    start: u64,
    /// Lit now: bit `column * LINES + line`.
    lit: u64,
    /// Selected now: bit per column.
    selected: u16,
    on: [[u64; LINES]; COLUMNS],
    selected_for: [u64; COLUMNS],
}

impl Leds {
    /// Device time moves on.
    #[inline]
    pub(crate) fn advance(&mut self, ticks: u32) {
        self.now += ticks as u64;
    }

    /// New pin states: `latched` is the 595 chain's output word (low =
    /// column selected), `lines` the LED lines driven high.
    #[inline]
    pub(crate) fn update(&mut self, latched: u16, lines: u8) {
        let selected = !latched & ((1 << COLUMNS) - 1);
        let mut lit = 0u64;
        if lines != 0 {
            let mut columns = selected;
            while columns != 0 {
                let column = columns.trailing_zeros() as usize;
                columns &= columns - 1;
                lit |= (lines as u64 & 0xf) << (column * LINES);
            }
        }
        if lit == self.lit && selected == self.selected {
            return;
        }
        self.accumulate();
        self.lit = lit;
        self.selected = selected;
    }

    fn accumulate(&mut self) {
        let span = self.now - self.since;
        self.since = self.now;
        if span == 0 {
            return;
        }
        let mut lit = self.lit;
        while lit != 0 {
            let bit = lit.trailing_zeros() as usize;
            lit &= lit - 1;
            self.on[bit / LINES][bit % LINES] += span;
        }
        let mut columns = self.selected;
        while columns != 0 {
            let column = columns.trailing_zeros() as usize;
            columns &= columns - 1;
            self.selected_for[column] += span;
        }
    }

    /// Device time (oscillator ticks) since the last `take`.
    pub fn window(&self) -> u64 {
        self.now - self.start
    }

    /// Brightness since the last call (or since reset), and start a new window.
    pub fn take(&mut self) -> Brightness {
        self.accumulate();
        let mut brightness = [[0.; LINES]; COLUMNS];
        for (column, levels) in brightness.iter_mut().enumerate() {
            let selected = self.selected_for[column];
            if selected != 0 {
                for (line, level) in levels.iter_mut().enumerate() {
                    *level = (self.on[column][line] as f64 / selected as f64) as f32;
                }
            }
        }
        self.on = Default::default();
        self.selected_for = Default::default();
        self.start = self.now;
        brightness
    }
}

/// Brightness per key id (buttons 0..13, note keys 14..40).
pub fn by_key(brightness: &Brightness) -> [f32; KEYS] {
    let mut keys = [0.; KEYS];
    for (line, ids) in KEYMAP.iter().enumerate() {
        for (column, &id) in ids.iter().enumerate() {
            if id >= 0 {
                keys[id as usize] = brightness[column][line];
            }
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLUMN_TICKS: u32 = 240; // 10 us per column at 24 MHz.

    /// One scan frame: each column selected for COLUMN_TICKS, its lines lit
    /// for `lit_ticks` of that.
    fn frame(leds: &mut Leds, lines: impl Fn(usize) -> u8, lit_ticks: u32) {
        for column in 0..COLUMNS {
            leds.update(!(1 << column), 0);
            leds.advance(COLUMN_TICKS - lit_ticks);
            leds.update(!(1 << column), lines(column));
            leds.advance(lit_ticks);
        }
        leds.update(u16::MAX, 0);
    }

    #[test]
    fn brightness_is_on_time_over_the_column_time() {
        let mut leds = Leds::default();
        // Column 3 line 2 full on, column 5 line 0 lit one frame in four.
        for n in 0..8 {
            frame(
                &mut leds,
                |c| match c {
                    3 => 4,
                    5 if n % 4 == 0 => 1,
                    _ => 0,
                },
                COLUMN_TICKS,
            );
        }
        let b = leds.take();
        assert_eq!(b[3][2], 1.);
        assert_eq!(b[5][0], 0.25);
        assert_eq!(b[0][0], 0.);
        let keys = by_key(&b);
        assert_eq!(keys[KEYMAP[2][3] as usize], 1.);
        assert_eq!(keys[KEYMAP[0][5] as usize], 0.25);
        // A new window starts dark.
        frame(&mut leds, |_| 0, COLUMN_TICKS);
        assert_eq!(leds.take(), [[0.; LINES]; COLUMNS]);
    }

    #[test]
    fn lines_off_while_the_rows_are_read_dim_the_full_level() {
        let mut leds = Leds::default();
        for _ in 0..4 {
            frame(&mut leds, |_| 0xf, COLUMN_TICKS * 3 / 4);
        }
        assert!(leds.take().iter().flatten().all(|&level| level == 0.75));
    }

    #[test]
    fn an_unscanned_column_or_an_undriven_line_stays_dark() {
        let mut leds = Leds::default();
        leds.update(u16::MAX, 0xf);
        leds.advance(100);
        leds.update(!1, 0);
        leds.advance(900);
        assert_eq!(leds.take(), [[0.; LINES]; COLUMNS]);
        leds.advance(500);
        assert_eq!(leds.window(), 500);
    }
}
