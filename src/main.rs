use std::{
    fs,
    io::{self, Write},
    time::Duration,
};

use crossterm::event;
use tui::{
    backend::CrosstermBackend,
    text::{Span, Spans},
    widgets, Terminal,
};

use roads::NominatimEntry;

struct State {
    user_city: String,
    cities_found: Vec<NominatimEntry>,
    cities_state: widgets::ListState,
}

impl State {
    fn new() -> Self {
        State {
            user_city: String::new(),
            cities_found: vec![],
            cities_state: widgets::ListState::default(),
        }
    }

    fn push_city_c(&mut self, c: char) {
        self.user_city.push(c);
        self.set_cities(vec![]);
    }

    fn pop_city_c(&mut self) {
        self.user_city.pop();
        self.set_cities(vec![]);
    }

    fn set_cities(&mut self, cities_found: Vec<NominatimEntry>) {
        self.cities_found = cities_found;
        self.cities_state = widgets::ListState::default();
        if !self.cities_found.is_empty() {
            self.cities_state.select(Some(0));
        }
    }

    fn select_next_city(&mut self) {
        if self.cities_found.is_empty() {
            return;
        }

        let next = (self.cities_state.selected().unwrap_or_default() + 1) % self.cities_found.len();
        self.cities_state.select(Some(next));
    }

    fn select_prev_city(&mut self) {
        if self.cities_found.is_empty() {
            return;
        }

        let next = (self.cities_state.selected().unwrap_or_default() + self.cities_found.len() - 1)
            % self.cities_found.len();
        self.cities_state.select(Some(next));
    }
}

fn main() -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;

    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = State::new();

    terminal.clear()?;
    loop {
        terminal.draw(|f| {
            use tui::{
                layout::{Constraint, Direction, Layout},
                style::{Color, Modifier, Style},
                widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
            };

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(95), Constraint::Percentage(5)].as_ref())
                .split(f.size());

            let city_input = Paragraph::new(state.user_city.as_ref())
                .block(Block::default().title("Search").borders(Borders::ALL))
                .wrap(Wrap { trim: true });

            let found_entries = List::new(
                state
                    .cities_found
                    .iter()
                    .map(|e| {
                        ListItem::new(Spans::from(vec![
                            Span::raw(e.display_name.clone()),
                            Span::styled(
                                format!(" ({} - {})", e.osm_id, e.osm_type),
                                Style::default().add_modifier(Modifier::ITALIC),
                            ),
                        ]))
                    })
                    .collect::<Vec<_>>(),
            )
            .block(Block::default().title("Places").borders(Borders::ALL))
            .highlight_symbol(">> ")
            .highlight_style(
                Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::ITALIC | Modifier::DIM),
            );

            f.render_stateful_widget(found_entries, chunks[0], &mut state.cities_state);
            f.render_widget(city_input, chunks[1]);
        })?;

        if event::poll(Duration::from_millis(500))? {
            use crossterm::event::{Event, KeyCode, KeyEvent};

            let KeyEvent { code, .. } = match event::read()? {
                Event::Key(k) => k,
                _ => continue,
            };

            match code {
                KeyCode::Esc => break,
                KeyCode::Backspace => {
                    state.pop_city_c();
                }
                KeyCode::Char(c) => {
                    state.push_city_c(c);
                }
                KeyCode::Up => {
                    state.select_prev_city();
                }
                KeyCode::Down => {
                    state.select_next_city();
                }
                KeyCode::Enter => match state.cities_state.selected() {
                    None => {
                        if !state.user_city.is_empty() {
                            let cities =
                                async_std::task::block_on(roads::search(&state.user_city)).unwrap();
                            state.set_cities(cities);
                        }
                    }
                    Some(ix) => {
                        let paths =
                            async_std::task::block_on(roads::fetch_roads(&state.cities_found[ix]))
                                .unwrap();
                        dump_svg(&format!("{}.svg", state.user_city), (1920.0, 1080.0), paths)?;
                        break;
                    }
                },
                _ => {}
            }
        }
    }

    terminal.clear()?;
    crossterm::terminal::disable_raw_mode()?;

    Ok(())
}

fn dump_svg(path: &str, (w, h): (f64, f64), mut paths: Vec<Vec<(f64, f64)>>) -> io::Result<()> {
    use std::f64::{INFINITY, NEG_INFINITY};

    let mut min_x = INFINITY;
    let mut min_y = INFINITY;
    let mut max_x = NEG_INFINITY;
    let mut max_y = NEG_INFINITY;

    for p in &mut paths {
        *p = roads::simplify::simplify(p);

        for (x, y) in p {
            min_x = x.min(min_x);
            min_y = y.min(min_y);
            max_x = x.max(max_x);
            max_y = y.max(max_y);
        }
    }

    if min_x > max_x || min_y > max_y {
        return Ok(());
    }

    let sf = f64::min(w / (max_x - min_x), h / (max_y - min_y));

    let f = fs::File::create(path)?;
    let mut f = io::BufWriter::new(f);

    writeln!(
        f,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {:.2} {:.2}">
<g stroke="black" stroke-width="0.1" fill="none" >"#,
        (max_x - min_x) * sf,
        (max_y - min_y) * sf,
    )?;

    for p in paths {
        write!(f, r#"<polyline points=""#)?;
        for (x, y) in p {
            write!(f, "{:.2}, {:.2} ", (x - min_x) * sf, h - (y - min_y) * sf)?;
        }
        writeln!(f, r#"" />"#)?;
    }

    writeln!(f, "</g>\n</svg>")?;

    Ok(())
}
