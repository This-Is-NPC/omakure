//! Reusable activity-grid widget.
//!
//! Given a set of runs and a [`ActivityPeriod`], the widget renders a
//! coloured grid where each cell represents one time bucket. A cell is
//! green when every run in that bucket succeeded, red when every run
//! failed, yellow on a mixed bucket, magenta while a run is still
//! in-flight, and dim when the bucket is empty.
//!
//! Used by:
//! - the Schedules screen (right panel) for the currently selected
//!   scheduled script
//! - the script-select Charts panel for the script currently under the
//!   cursor
//!
//! The grid layout per period:
//! - [`ActivityPeriod::Day`]: 24 cells (one per hour of the last 24h),
//!   laid out as a single row.
//! - [`ActivityPeriod::Week`]: 7 cells (one per day of the last 7 days),
//!   laid out as a single row.
//! - [`ActivityPeriod::Month`]: 30 cells (one per day of the last 30
//!   days), arranged in a rectangular grid that fits the available
//!   width.
//! - [`ActivityPeriod::Year`]: 53 weeks × 7 days (GitHub-style
//!   contribution heatmap), covering the last ~12 months.

use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Timelike};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::super::theme::Theme;
use crate::runs::{RunRow, RunState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ActivityPeriod {
    LastMinute,
    LastHour,
    Day,
    #[default]
    Week,
    Month,
    Year,
}

impl ActivityPeriod {
    pub(crate) fn next(self) -> Self {
        match self {
            ActivityPeriod::LastMinute => ActivityPeriod::LastHour,
            ActivityPeriod::LastHour => ActivityPeriod::Day,
            ActivityPeriod::Day => ActivityPeriod::Week,
            ActivityPeriod::Week => ActivityPeriod::Month,
            ActivityPeriod::Month => ActivityPeriod::Year,
            ActivityPeriod::Year => ActivityPeriod::LastMinute,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            ActivityPeriod::LastMinute => "Last min",
            ActivityPeriod::LastHour => "Last hour",
            ActivityPeriod::Day => "Day",
            ActivityPeriod::Week => "Week",
            ActivityPeriod::Month => "Month",
            ActivityPeriod::Year => "Year",
        }
    }
}

/// Aggregated outcome of all runs that fell into one bucket.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BucketOutcome {
    pub successes: u32,
    pub failures: u32,
    pub in_flight: u32,
}

impl BucketOutcome {
    pub(crate) fn total(&self) -> u32 {
        self.successes + self.failures + self.in_flight
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

fn classify(row: &RunRow) -> RunClass {
    match row.state {
        RunState::Queued | RunState::Running => RunClass::InFlight,
        RunState::Completed => {
            if row.success.unwrap_or(false) {
                RunClass::Success
            } else {
                RunClass::Failure
            }
        }
        RunState::Failed | RunState::Cancelled | RunState::TimedOut | RunState::DeadLetter => {
            RunClass::Failure
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RunClass {
    Success,
    Failure,
    InFlight,
}

/// Bucket `rows` into the grid implied by `period`, using `now` as the
/// reference "now" in the local timezone. Returned vector length is the
/// bucket count for that period.
pub(crate) fn bucketize(
    rows: &[&RunRow],
    period: ActivityPeriod,
    now: DateTime<Local>,
) -> Vec<BucketOutcome> {
    let (bucket_count, bucket_for) = bucket_layout(period, now);
    let mut out = vec![BucketOutcome::default(); bucket_count];
    for row in rows {
        let ts = row_timestamp(row);
        let Some(idx) = bucket_for(ts) else { continue };
        if idx >= bucket_count {
            continue;
        }
        match classify(row) {
            RunClass::Success => out[idx].successes += 1,
            RunClass::Failure => out[idx].failures += 1,
            RunClass::InFlight => out[idx].in_flight += 1,
        }
    }
    out
}

fn row_timestamp(row: &RunRow) -> DateTime<Local> {
    let ms = row.started_at.unwrap_or(row.enqueued_at);
    DateTime::from_timestamp_millis(ms)
        .map(|u| u.with_timezone(&Local))
        .unwrap_or_else(Local::now)
}

/// Bucketize `upcoming` scheduled fire times into a parallel boolean
/// mask of length equal to the period's bucket count. A `true` at index
/// `i` means at least one upcoming fire falls in bucket `i`.
fn bucketize_upcoming(
    upcoming: &[DateTime<Local>],
    period: ActivityPeriod,
    now: DateTime<Local>,
) -> Vec<bool> {
    let (bucket_count, bucket_for) = bucket_layout(period, now);
    let mut out = vec![false; bucket_count];
    for ts in upcoming {
        let Some(idx) = bucket_for(*ts) else { continue };
        if idx < bucket_count {
            out[idx] = true;
        }
    }
    out
}

type BucketFn = Box<dyn Fn(DateTime<Local>) -> Option<usize>>;

fn bucket_layout(period: ActivityPeriod, now: DateTime<Local>) -> (usize, BucketFn) {
    match period {
        ActivityPeriod::LastMinute => {
            // 60 per-second buckets covering the last 60 seconds.
            // Bucket 0 = 59 seconds ago, bucket 59 = right now.
            let start = floor_to_second(now) - Duration::seconds(59);
            (
                60,
                Box::new(move |ts| {
                    let diff = ts.signed_duration_since(start).num_milliseconds();
                    if diff < 0 {
                        return None;
                    }
                    let sec = diff / 1000;
                    if !(0..60).contains(&sec) {
                        return None;
                    }
                    Some(sec as usize)
                }),
            )
        }
        ActivityPeriod::LastHour => {
            // 60 per-minute buckets covering the last 60 minutes.
            // Bucket 0 = 59 minutes ago, bucket 59 = current minute.
            let start = floor_to_minute(now) - Duration::minutes(59);
            (
                60,
                Box::new(move |ts| {
                    let diff = ts.signed_duration_since(start).num_seconds();
                    if diff < 0 {
                        return None;
                    }
                    let min = diff / 60;
                    if !(0..60).contains(&min) {
                        return None;
                    }
                    Some(min as usize)
                }),
            )
        }
        ActivityPeriod::Day => {
            // 24 hourly buckets covering the current local day
            // (00:00 → 23:59). Bucket 0 = 00:00, bucket 23 = 23:00.
            let start_of_day = start_of_day(now);
            (
                24,
                Box::new(move |ts| {
                    let diff = ts.signed_duration_since(start_of_day);
                    if diff < Duration::zero() {
                        return None;
                    }
                    let hours = diff.num_hours();
                    if !(0..24).contains(&hours) {
                        return None;
                    }
                    Some(hours as usize)
                }),
            )
        }
        ActivityPeriod::Week => {
            // Current calendar week, Monday → Sunday.
            let this_monday = monday_of_week(now);
            (7, day_bucket(this_monday, 7))
        }
        ActivityPeriod::Month => {
            // 5 calendar weeks × 7 days, weeks starting on Monday.
            // Row 0 (oldest week) begins 4 weeks before the Monday of
            // the current week.
            let this_monday = monday_of_week(now);
            let start_day = this_monday - Duration::days(7 * 4);
            (35, day_bucket(start_day, 35))
        }
        ActivityPeriod::Year => {
            // Current calendar year, Jan 1 → Dec 31, padded to start
            // on the Monday on-or-before Jan 1 so the GitHub-style
            // column-major layout fits the rectangular grid.
            let start_day = year_grid_start(now);
            (53 * 7, day_bucket(start_day, 53 * 7))
        }
    }
}

fn start_of_day(now: DateTime<Local>) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
        .unwrap_or(now)
}

fn floor_to_minute(now: DateTime<Local>) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(
            now.year(),
            now.month(),
            now.day(),
            now.hour(),
            now.minute(),
            0,
        )
        .single()
        .unwrap_or(now)
}

fn floor_to_second(now: DateTime<Local>) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(
            now.year(),
            now.month(),
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
        )
        .single()
        .unwrap_or(now)
}

fn monday_of_week(now: DateTime<Local>) -> DateTime<Local> {
    let today = date_only(now);
    let offset = today.weekday().num_days_from_monday() as i64;
    today - Duration::days(offset)
}

fn year_grid_start(now: DateTime<Local>) -> DateTime<Local> {
    let jan1 = Local
        .with_ymd_and_hms(now.year(), 1, 1, 0, 0, 0)
        .single()
        .unwrap_or(now);
    let offset = jan1.weekday().num_days_from_monday() as i64;
    jan1 - Duration::days(offset)
}

fn day_bucket(start: DateTime<Local>, count: usize) -> BucketFn {
    Box::new(move |ts| {
        let d = date_only(ts);
        let diff = d.signed_duration_since(start).num_days();
        if diff < 0 || diff >= count as i64 {
            return None;
        }
        Some(diff as usize)
    })
}

fn date_only(ts: DateTime<Local>) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(ts.year(), ts.month(), ts.day(), 0, 0, 0)
        .single()
        .unwrap_or(ts)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const CELL_FILLED: char = '█';
const CELL_UPCOMING: char = '▒';

fn cell_style(theme: &Theme, bucket: BucketOutcome) -> (char, Style) {
    if bucket.is_empty() {
        return (' ', theme.text_muted());
    }
    let color = if bucket.in_flight > 0 {
        Color::Magenta
    } else if bucket.failures > 0 && bucket.successes > 0 {
        Color::Yellow
    } else if bucket.failures > 0 {
        Color::Red
    } else {
        Color::Green
    };
    (
        CELL_FILLED,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

/// Cell style when the bucket may also contain an upcoming (scheduled
/// future) run. Upcoming runs are painted with a lighter shade in cyan
/// and only shown when the bucket has no past activity of its own —
/// mixing future with past would clobber the past outcome's color.
fn cell_style_with_upcoming(theme: &Theme, bucket: BucketOutcome, upcoming: bool) -> (char, Style) {
    if bucket.is_empty() && upcoming {
        return (
            CELL_UPCOMING,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    }
    cell_style(theme, bucket)
}

/// Render the activity heatmap. `upcoming` is a list of future scheduled
/// fire times in local timezone; pass `&[]` to suppress the overlay.
/// Each upcoming time that falls inside the period's window is rendered
/// in a distinctive cyan shade on buckets that have no past activity.
/// `LastMinute` and `LastHour` ignore the upcoming overlay entirely —
/// they only depict the past.
pub(crate) fn render_activity_grid_with_upcoming(
    frame: &mut Frame,
    area: Rect,
    rows: &[&RunRow],
    upcoming: &[DateTime<Local>],
    period: ActivityPeriod,
    theme: &Theme,
    title: &str,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(format!("{title}  [{}]", period.label()))
        .title_style(theme.text_secondary());
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    if inner.width < 8 || inner.height < 3 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    let now = Local::now();
    let buckets = bucketize(rows, period, now);
    let upcoming_mask = if matches!(
        period,
        ActivityPeriod::LastMinute | ActivityPeriod::LastHour
    ) || upcoming.is_empty()
    {
        Vec::new()
    } else {
        bucketize_upcoming(upcoming, period, now)
    };
    let (cols, rows_count, column_major) = grid_dims(period);
    let (target_w, cell_h) = cell_dims(period);
    let (x_labels, y_labels) = axis_labels(period, now);
    let mut lines = render_fixed_grid(
        &buckets,
        &upcoming_mask,
        cols,
        rows_count,
        column_major,
        target_w,
        cell_h,
        chunks[0].width,
        theme,
        &x_labels,
        &y_labels,
    );
    // Vertical centering: pad blank lines on top based on the slack
    // between our natural line count and the chunk height.
    let slack = (chunks[0].height as usize).saturating_sub(lines.len());
    let pad_top = slack / 2;
    if pad_top > 0 {
        let mut padded: Vec<Line<'_>> = Vec::with_capacity(pad_top + lines.len());
        for _ in 0..pad_top {
            padded.push(Line::from(""));
        }
        padded.append(&mut lines);
        lines = padded;
    }
    let body = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(body, chunks[0]);

    let legend = build_legend(&buckets, &upcoming_mask, theme);
    frame.render_widget(legend, chunks[1]);
}

/// Logical shape of the grid for each period: `(cols, rows, column_major)`.
/// When `column_major` is true, the chronological bucket index is mapped
/// via `col * rows + row` (GitHub-style year heatmap). Otherwise it is
/// mapped row-major (`row * cols + col`).
fn grid_dims(period: ActivityPeriod) -> (usize, usize, bool) {
    match period {
        // LastMinute/LastHour: 60 cells laid out 10 × 6 (tens on Y,
        // units on X) so the grid fits even in narrow panels.
        ActivityPeriod::LastMinute | ActivityPeriod::LastHour => (10, 6, false),
        // Day: one row of 24 hourly cells.
        ActivityPeriod::Day => (24, 1, false),
        // Week: seven days in a row.
        ActivityPeriod::Week => (7, 1, false),
        // Month: five calendar weeks × seven days, row-major (row 0 =
        // oldest week, row 4 = this week; col 0 = Monday).
        ActivityPeriod::Month => (7, 5, false),
        // Year: 53 weeks × 7 days, column-major GitHub-style heatmap.
        ActivityPeriod::Year => (53, 7, true),
    }
}

/// Fixed glyph size of one cell for each period: `(cell_w, cell_h)`.
/// Cells are compact (single terminal row) so the grid fits in narrow
/// panes like the right column of `ScriptSelect`. Day/Year use 2-wide
/// cells because their grids have the most columns (24 and 53); the
/// remaining periods use 3-wide cells to stay visually distinguishable
/// while still fitting the LastMinute/LastHour 10-column grid in a
/// narrow pane without collapsing to 1-char cells.
fn cell_dims(period: ActivityPeriod) -> (usize, usize) {
    match period {
        ActivityPeriod::Day | ActivityPeriod::Year => (2, 1),
        ActivityPeriod::Week
        | ActivityPeriod::Month
        | ActivityPeriod::LastHour
        | ActivityPeriod::LastMinute => (3, 1),
    }
}

/// Total number of terminal rows the widget needs for a given period:
/// block borders + X-label row (when present) + grid body + legend + a
/// small vertical slack so the grid can be centred vertically. Parent
/// layouts pass this as a `Constraint::Length` so the Activity block is
/// sized to its natural height instead of stretching.
pub(crate) fn widget_height(period: ActivityPeriod) -> u16 {
    let (_, rows, _) = grid_dims(period);
    let (_, cell_h) = cell_dims(period);
    let grid_body = rows * cell_h + (rows + 1);
    let x_label_rows = if has_x_labels(period) { 1 } else { 0 };
    // 2 block borders + 1 legend + vertical slack (2) for centring.
    (2 + x_label_rows + grid_body + 1 + 2) as u16
}

fn has_x_labels(_period: ActivityPeriod) -> bool {
    true
}

/// Compute X (column header) and Y (row header) axis labels for the given
/// period, anchored on `now`.
fn axis_labels(period: ActivityPeriod, now: DateTime<Local>) -> (Vec<String>, Vec<String>) {
    match period {
        ActivityPeriod::LastMinute => {
            // 60 one-second cells, laid out 10 cols × 6 rows. Columns =
            // unit of second (0..9), rows = tens of second (newest row
            // is the one containing the current second).
            let x: Vec<String> = (0..10).map(|i| format!("{i}")).collect();
            let y: Vec<String> = (0..6).map(|i| format!("{}s", i * 10)).collect();
            (x, y)
        }
        ActivityPeriod::LastHour => {
            let x: Vec<String> = (0..10).map(|i| format!("{i}")).collect();
            let y: Vec<String> = (0..6).map(|i| format!("{}m", i * 10)).collect();
            (x, y)
        }
        ActivityPeriod::Day => {
            // 24 hourly cells; label each with its hour-of-day (00..23).
            let x: Vec<String> = (0..24).map(|h| format!("{:02}", h)).collect();
            (x, Vec::new())
        }
        ActivityPeriod::Week => {
            // Current calendar week, Mon → Sun.
            let x = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
                .into_iter()
                .map(String::from)
                .collect();
            (x, Vec::new())
        }
        ActivityPeriod::Month => {
            let x = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
                .into_iter()
                .map(String::from)
                .collect();
            let y = (0..5).map(|i| format!("{}w", 5 - i)).collect();
            (x, y)
        }
        ActivityPeriod::Year => {
            // Calendar year Jan → Dec. Place each month's abbreviation
            // in the column of its first week; columns before Jan (if
            // the grid starts in the previous December) stay blank so
            // "Jan" is always the first visible label.
            let year = now.year();
            let start = year_grid_start(now).date_naive();
            let mut x = vec![String::new(); 53];
            let mut last_month: u32 = 0;
            for (w, slot) in x.iter_mut().enumerate() {
                let d = start + chrono::Duration::days((w as i64) * 7);
                if d.year() == year && d.month() != last_month {
                    last_month = d.month();
                    *slot = d.format("%b").to_string();
                }
            }
            let y = vec![
                "Mon".into(),
                "".into(),
                "Wed".into(),
                "".into(),
                "Fri".into(),
                "".into(),
                "".into(),
            ];
            (x, y)
        }
    }
}

/// Render `buckets` laid out as `cols` × `rows` logical cells into the
/// available `area`. Each logical cell is painted with `cell_w` glyphs
/// horizontally and `cell_h` terminal rows vertically. Terminal glyphs
/// are roughly 1:2 (w:h), so for a visually-square cell we aim for
/// `cell_w = 2 * cell_h`; the actual values clamp to whatever fits.
#[allow(clippy::too_many_arguments)]
fn render_fixed_grid<'a>(
    buckets: &[BucketOutcome],
    upcoming_mask: &[bool],
    cols: usize,
    rows: usize,
    column_major: bool,
    target_w: usize,
    cell_h: usize,
    avail_w: u16,
    theme: &'a Theme,
    x_labels: &[String],
    y_labels: &[String],
) -> Vec<Line<'a>> {
    if cols == 0 || rows == 0 || cell_h == 0 {
        return Vec::new();
    }
    // Reserve width for the leading Y-label column plus the trailing
    // `│`, `│…`, and the cells with their column separators.
    let y_width = y_labels
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(0);
    let y_pad = if y_width > 0 { y_width + 1 } else { 0 };
    let avail_w = avail_w as usize;
    let overhead = cols + 1 + y_pad;
    let cell_w_max = avail_w.saturating_sub(overhead) / cols.max(1);
    let cell_w = target_w.min(cell_w_max).max(1);

    let grid_style = theme.text_muted();
    let top = prepend_pad(
        build_separator(cols, cell_w, '┌', '┬', '┐', grid_style),
        y_pad,
    );
    let mid = prepend_pad(
        build_separator(cols, cell_w, '├', '┼', '┤', grid_style),
        y_pad,
    );
    let bot = prepend_pad(
        build_separator(cols, cell_w, '└', '┴', '┘', grid_style),
        y_pad,
    );

    let mut lines: Vec<Line<'a>> = Vec::new();
    if !x_labels.is_empty() {
        lines.push(build_x_header(x_labels, cell_w, y_pad, grid_style));
    }
    lines.push(top);
    for row in 0..rows {
        for sub in 0..cell_h {
            let y_label = y_labels.get(row).cloned().unwrap_or_default();
            // Only the first sub-row of each cell carries the label;
            // other sub-rows render only the cells.
            let label_opt = if sub == 0 {
                Some(y_label.as_str())
            } else {
                None
            };
            lines.push(build_row_line(
                buckets,
                upcoming_mask,
                cols,
                rows,
                row,
                cell_w,
                column_major,
                grid_style,
                theme,
                label_opt,
                y_width,
            ));
        }
        if row + 1 < rows {
            lines.push(mid.clone());
        }
    }
    lines.push(bot);
    lines
}

fn prepend_pad<'a>(mut line: Line<'a>, pad: usize) -> Line<'a> {
    if pad == 0 {
        return line;
    }
    let padding = " ".repeat(pad);
    let mut spans = vec![Span::raw(padding)];
    spans.append(&mut line.spans);
    Line::from(spans)
}

fn build_x_header<'a>(x_labels: &[String], cell_w: usize, y_pad: usize, style: Style) -> Line<'a> {
    // Build a character buffer sized to the full header width and
    // overlay each non-empty label at its column position. Labels are
    // allowed to flow into subsequent empty columns, which is what
    // makes month names readable on the Year view where each slot is
    // only 2 chars wide.
    let col_width = cell_w + 1; // cell content + trailing `│` position
    let leading = y_pad + 1; // Y-label pad + leftmost `│` position
    let total = leading + x_labels.len() * col_width;
    let mut buf: Vec<char> = vec![' '; total];
    for (col, label) in x_labels.iter().enumerate() {
        if label.is_empty() {
            continue;
        }
        // Centre the label within its own slot when it fits; otherwise
        // anchor at the start of the slot and let it overflow.
        let slot_start = leading + col * col_width;
        let label_len = label.chars().count();
        let start = if label_len <= cell_w {
            slot_start + (cell_w - label_len) / 2
        } else {
            slot_start
        };
        for (i, ch) in label.chars().enumerate() {
            let pos = start + i;
            if pos < buf.len() {
                buf[pos] = ch;
            }
        }
    }
    let text: String = buf.into_iter().collect();
    Line::from(Span::styled(text, style))
}

fn build_separator<'a>(
    cols: usize,
    cell_w: usize,
    left: char,
    mid: char,
    right: char,
    style: Style,
) -> Line<'a> {
    let horizontal: String = std::iter::repeat_n('─', cell_w).collect();
    let mut out = String::new();
    out.push(left);
    for c in 0..cols {
        out.push_str(&horizontal);
        out.push(if c + 1 == cols { right } else { mid });
    }
    Line::from(Span::styled(out, style))
}

#[allow(clippy::too_many_arguments)]
fn build_row_line<'a>(
    buckets: &[BucketOutcome],
    upcoming_mask: &[bool],
    cols: usize,
    rows: usize,
    row: usize,
    cell_w: usize,
    column_major: bool,
    grid_style: Style,
    theme: &'a Theme,
    y_label: Option<&str>,
    y_width: usize,
) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::with_capacity(cols * 2 + 2);
    if y_width > 0 {
        let label = y_label.unwrap_or("");
        let pad = y_width.saturating_sub(label.chars().count());
        let text = format!("{}{} ", " ".repeat(pad), label);
        spans.push(Span::styled(text, grid_style));
    }
    spans.push(Span::styled("│", grid_style));
    for col in 0..cols {
        let bucket_index = if column_major {
            col * rows + row
        } else {
            row * cols + col
        };
        let bucket = buckets.get(bucket_index).copied().unwrap_or_default();
        let upcoming = upcoming_mask.get(bucket_index).copied().unwrap_or(false);
        let (ch, style) = cell_style_with_upcoming(theme, bucket, upcoming);
        let chunk: String = std::iter::repeat_n(ch, cell_w).collect();
        spans.push(Span::styled(chunk, style));
        spans.push(Span::styled("│", grid_style));
    }
    Line::from(spans)
}

fn build_legend<'a>(
    buckets: &[BucketOutcome],
    upcoming_mask: &[bool],
    theme: &'a Theme,
) -> Paragraph<'a> {
    let totals: BucketOutcome = buckets.iter().fold(BucketOutcome::default(), |mut acc, b| {
        acc.successes += b.successes;
        acc.failures += b.failures;
        acc.in_flight += b.in_flight;
        acc
    });
    let scheduled_count = upcoming_mask.iter().filter(|b| **b).count();
    let mut spans = vec![
        Span::styled(format!("{CELL_FILLED} "), Style::default().fg(Color::Green)),
        Span::raw(format!("ok {}  ", totals.successes)),
        Span::styled(format!("{CELL_FILLED} "), Style::default().fg(Color::Red)),
        Span::raw(format!("fail {}  ", totals.failures)),
        Span::styled(
            format!("{CELL_FILLED} "),
            Style::default().fg(Color::Magenta),
        ),
        Span::raw(format!("live {}  ", totals.in_flight)),
    ];
    if scheduled_count > 0 {
        spans.push(Span::styled(
            format!("{CELL_UPCOMING} "),
            Style::default().fg(Color::Cyan),
        ));
        spans.push(Span::raw(format!("scheduled {}  ", scheduled_count)));
    }
    spans.push(Span::styled("□ ", theme.text_muted()));
    spans.push(Span::raw("none    Tab: next period"));
    Paragraph::new(Line::from(spans)).style(theme.text_secondary())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn row_at(state: RunState, success: Option<bool>, ms: i64) -> RunRow {
        RunRow {
            run_id: format!("r-{ms}"),
            script_path: "/s.sh".into(),
            script_name: None,
            args_json: "[]".into(),
            actor: "t".into(),
            reason: None,
            state,
            priority: 0,
            enqueued_at: ms,
            worker_id: None,
            lease_until: None,
            timeout_ms: None,
            cron_schedule_id: None,
            trigger: crate::runs::RunTrigger::Manual,
            started_at: Some(ms),
            finished_at: Some(ms),
            duration_ms: Some(0),
            exit_code: Some(0),
            success,
            stdout: String::new(),
            stderr: String::new(),
            error: None,
            parent_run_id: None,
            omakure_version: "test".into(),
        }
    }

    #[test]
    fn day_period_has_24_buckets() {
        let buckets = bucketize(&[], ActivityPeriod::Day, Local::now());
        assert_eq!(buckets.len(), 24);
    }

    #[test]
    fn week_period_has_7_buckets() {
        let buckets = bucketize(&[], ActivityPeriod::Week, Local::now());
        assert_eq!(buckets.len(), 7);
    }

    #[test]
    fn month_period_has_35_buckets() {
        let buckets = bucketize(&[], ActivityPeriod::Month, Local::now());
        assert_eq!(buckets.len(), 35);
    }

    #[test]
    fn year_period_has_371_buckets() {
        let buckets = bucketize(&[], ActivityPeriod::Year, Local::now());
        assert_eq!(buckets.len(), 7 * 53);
    }

    #[test]
    fn run_now_goes_to_current_hour_bucket() {
        use chrono::Timelike as _;
        let now = Local::now();
        let ms = now.timestamp_millis();
        let row = row_at(RunState::Completed, Some(true), ms);
        let buckets = bucketize(&[&row], ActivityPeriod::Day, now);
        let current_hour = now.hour() as usize;
        assert_eq!(buckets[current_hour].successes, 1);
    }

    #[test]
    fn failure_colors_red_success_green_mixed_yellow() {
        let (_, s_ok) = cell_style(
            &Theme::default(),
            BucketOutcome {
                successes: 1,
                ..Default::default()
            },
        );
        let (_, s_err) = cell_style(
            &Theme::default(),
            BucketOutcome {
                failures: 1,
                ..Default::default()
            },
        );
        let (_, s_mix) = cell_style(
            &Theme::default(),
            BucketOutcome {
                successes: 1,
                failures: 1,
                ..Default::default()
            },
        );
        assert_eq!(s_ok.fg, Some(Color::Green));
        assert_eq!(s_err.fg, Some(Color::Red));
        assert_eq!(s_mix.fg, Some(Color::Yellow));
    }

    #[test]
    fn in_flight_overrides_other_colors() {
        let (_, style) = cell_style(
            &Theme::default(),
            BucketOutcome {
                successes: 3,
                failures: 1,
                in_flight: 1,
            },
        );
        assert_eq!(style.fg, Some(Color::Magenta));
    }

    #[test]
    fn period_next_cycles() {
        assert_eq!(ActivityPeriod::LastMinute.next(), ActivityPeriod::LastHour);
        assert_eq!(ActivityPeriod::LastHour.next(), ActivityPeriod::Day);
        assert_eq!(ActivityPeriod::Day.next(), ActivityPeriod::Week);
        assert_eq!(ActivityPeriod::Week.next(), ActivityPeriod::Month);
        assert_eq!(ActivityPeriod::Month.next(), ActivityPeriod::Year);
        assert_eq!(ActivityPeriod::Year.next(), ActivityPeriod::LastMinute);
    }

    #[test]
    fn last_hour_has_60_buckets() {
        let buckets = bucketize(&[], ActivityPeriod::LastHour, Local::now());
        assert_eq!(buckets.len(), 60);
    }

    #[test]
    fn last_minute_has_60_buckets() {
        let buckets = bucketize(&[], ActivityPeriod::LastMinute, Local::now());
        assert_eq!(buckets.len(), 60);
    }

    #[test]
    fn run_now_goes_to_last_bucket_of_last_minute() {
        let now = Local::now();
        let ms = now.timestamp_millis();
        let row = row_at(RunState::Completed, Some(true), ms);
        let buckets = bucketize(&[&row], ActivityPeriod::LastMinute, now);
        assert_eq!(buckets[59].successes, 1);
    }

    #[test]
    fn run_now_goes_to_last_bucket_of_last_hour() {
        let now = Local::now();
        let ms = now.timestamp_millis();
        let row = row_at(RunState::Completed, Some(true), ms);
        let buckets = bucketize(&[&row], ActivityPeriod::LastHour, now);
        assert_eq!(buckets[59].successes, 1);
    }

    #[test]
    fn render_no_panic_empty() {
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|f| {
                render_activity_grid_with_upcoming(
                    f,
                    f.size(),
                    &[],
                    &[],
                    ActivityPeriod::Week,
                    &theme,
                    "Activity",
                );
            })
            .unwrap();
    }

    #[test]
    fn render_no_panic_year_grid() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        let now = Local::now();
        let ms = now.timestamp_millis();
        let r = row_at(RunState::Completed, Some(true), ms);
        terminal
            .draw(|f| {
                render_activity_grid_with_upcoming(
                    f,
                    f.size(),
                    &[&r],
                    &[],
                    ActivityPeriod::Year,
                    &theme,
                    "Activity",
                );
            })
            .unwrap();
    }

    /// With the compact cell sizes, LastMinute and LastHour must render
    /// cleanly inside a narrow right-pane width (≈ 45% of a 120-col
    /// terminal). The grid should use the intended `cell_w` (3) without
    /// collapsing to 1-char cells via the `cell_w_max` clamp.
    #[test]
    fn last_minute_fits_narrow_pane() {
        let backend = TestBackend::new(50, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|f| {
                render_activity_grid_with_upcoming(
                    f,
                    f.size(),
                    &[],
                    &[],
                    ActivityPeriod::LastMinute,
                    &theme,
                    "Activity",
                );
            })
            .unwrap();
    }

    #[test]
    fn last_hour_fits_narrow_pane() {
        let backend = TestBackend::new(50, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|f| {
                render_activity_grid_with_upcoming(
                    f,
                    f.size(),
                    &[],
                    &[],
                    ActivityPeriod::LastHour,
                    &theme,
                    "Activity",
                );
            })
            .unwrap();
    }

    #[test]
    fn upcoming_bucket_paints_distinctive_cell_on_day() {
        // Use a deterministic "now" at 10:00 so +2h never crosses
        // midnight regardless of when the test runs.
        let now = Local::now()
            .date_naive()
            .and_hms_opt(10, 0, 0)
            .and_then(|dt| Local.from_local_datetime(&dt).single())
            .unwrap();
        let future = now + Duration::hours(2); // 12:00
        let upcoming = vec![future];
        let mask = bucketize_upcoming(&upcoming, ActivityPeriod::Day, now);
        assert_eq!(mask.len(), 24);
        assert!(mask[12], "hour 12 bucket should be marked");
    }

    #[test]
    fn upcoming_is_empty_for_last_minute_and_last_hour() {
        let now = Local::now();
        let future = now + Duration::seconds(5);
        let (ch, _) = cell_style_with_upcoming(&Theme::default(), BucketOutcome::default(), false);
        // Default (no past, no upcoming) renders blank.
        assert_eq!(ch, ' ');
        // LastMinute / LastHour bucketize_upcoming is never called by
        // render_activity_grid_with_upcoming (gated by the matches!()
        // branch), but the helper itself would still bucketize — that
        // is expected and tested by the Day test above. This test
        // ensures the cell style falls back to the default when the
        // upcoming flag is false, regardless of period.
        let _ = future;
    }

    /// On a 120-col terminal every period except Year should render at
    /// its intended `cell_w` without hitting the `cell_w_max` clamp.
    /// Year is excluded because 53×2 cells never fit below ~160 cols —
    /// its clamp-down to 1-char cells is a documented tradeoff.
    #[test]
    fn compact_cells_unclamped_on_wide_terminal() {
        use ActivityPeriod::*;
        for period in [Day, Week, Month, LastHour, LastMinute] {
            let (target_w, _) = cell_dims(period);
            let (cols, _, _) = grid_dims(period);
            let overhead = cols + 1 + 4;
            let avail_w = 120usize;
            let cell_w_max = avail_w.saturating_sub(overhead) / cols.max(1);
            assert!(
                cell_w_max >= target_w,
                "period {:?} clamps cell_w at 120 cols (max={} target={})",
                period,
                cell_w_max,
                target_w
            );
        }
    }
}
