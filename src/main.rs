use std::{
    fs,
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use crossterm::event;

use async_std::prelude::*;

use tui::{
    backend::{Backend, CrosstermBackend},
    text::{Span, Spans},
    Frame, Terminal,
};

use roads::{
    util::{DotsSpinner, WrappingList},
    NominatimEntry,
};

struct State {
    focus: WidgetId,
    user_city: String,
    places: WrappingList<NominatimEntry>,
    options: WrappingList<(&'static str, f64)>,
    worker_state: WorkerState,
    fetching_spinner: DotsSpinner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WidgetId {
    Places,
    Search,
    Options,
    Keybindings,
    Error,
}

enum WorkerState {
    Idle,
    Fetching,
    Error(anyhow::Error),
}

impl State {
    const WIDTH_OPTION: &'static str = "Width";
    const HEIGHT_OPTION: &'static str = "Height";
    const STROKE_WIDTH_OPTION: &'static str = "Line width";

    fn new() -> Self {
        State {
            focus: WidgetId::Search,
            user_city: String::new(),
            places: WrappingList::new(vec![]),
            options: WrappingList::new(vec![
                (Self::WIDTH_OPTION, 1920.0),
                (Self::HEIGHT_OPTION, 1080.0),
                (Self::STROKE_WIDTH_OPTION, 0.1),
            ]),
            worker_state: WorkerState::Idle,
            fetching_spinner: DotsSpinner::new(),
        }
    }

    fn worker_busy(&self) -> bool {
        match self.worker_state {
            WorkerState::Fetching => true,
            WorkerState::Idle | WorkerState::Error(_) => false,
        }
    }

    fn max_option_key_len(&self) -> usize {
        self.options
            .iter()
            .map(|(k, _)| k.len())
            .max()
            .unwrap_or_default()
    }

    fn option(&self, key: &str) -> Option<f64> {
        self.options
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, v)| *v)
    }

    fn fetch<T: Send + 'static>(
        &mut self,
        state: Arc<Mutex<Self>>,
        fut: impl Future<Output = anyhow::Result<T>> + Send + 'static,
        mut on_success: impl FnMut(&mut Self, T) -> anyhow::Result<()> + Send + 'static,
    ) {
        self.worker_state = WorkerState::Fetching;
        self.fetching_spinner = DotsSpinner::new();

        let _complete = async_std::task::spawn(async move {
            let err = |st: &mut State, e| {
                st.worker_state = WorkerState::Error(e);
                st.focus = WidgetId::Error;
                st.fetching_spinner = DotsSpinner::new();
            };

            match fut.await {
                Ok(d) => {
                    let mut state = state.lock().unwrap();
                    state.worker_state = WorkerState::Idle;
                    if let Err(e) = on_success(&mut state, d) {
                        err(&mut state, e);
                    }
                }
                Err(e) => {
                    let mut state = state.lock().unwrap();
                    err(&mut state, e);
                }
            }
        });
    }
}

async fn main_loop(terminal: &mut Terminal<impl Backend>) -> anyhow::Result<()> {
    use event::{Event, EventStream, KeyCode, KeyEvent};

    let mut reader = EventStream::new();
    let state = Arc::new(Mutex::new(State::new()));

    loop {
        terminal.draw(|f| {
            let mut state = state.lock().unwrap();
            draw(f, &mut state)
        })?;

        let ev = match async_std::future::timeout(Duration::from_millis(50), reader.next()).await {
            Err(_) => {
                // timeout expired
                continue;
            }
            Ok(ev) => ev,
        };

        match ev {
            Some(Ok(event)) => {
                let KeyEvent { code, .. } = match event {
                    Event::Key(k) => k,
                    _ => continue,
                };

                if code == KeyCode::Esc {
                    break;
                }

                if code == KeyCode::Tab || code == KeyCode::BackTab {
                    let mut state = state.lock().unwrap();

                    let tab_order = [
                        WidgetId::Search,
                        WidgetId::Places,
                        WidgetId::Options,
                        WidgetId::Keybindings,
                    ];
                    let current = tab_order.iter().position(|w| w == &state.focus).unwrap();
                    let next = current
                        + if code == KeyCode::Tab {
                            1
                        } else {
                            tab_order.len() - 1
                        };

                    state.focus = tab_order[next % tab_order.len()];
                    continue;
                }

                handle_key_event(code, &state).await?;
            }
            Some(Err(_)) | None => break,
        }
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;

    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    terminal.clear()?;

    async_std::task::block_on(main_loop(&mut terminal))?;

    terminal.clear()?;
    crossterm::terminal::disable_raw_mode()?;

    Ok(())
}

fn draw(f: &mut Frame<impl Backend>, state: &mut State) {
    use tui::{
        layout::{Constraint, Direction, Layout},
        style::{Color, Modifier, Style},
        widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    };

    let focus = state.focus;
    let block = |widget, title| {
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if focus == widget {
                Style::default().fg(Color::LightYellow)
            } else {
                Style::default()
            })
    };

    let list = |widget, title, symbol, items| {
        List::new(items)
            .block(block(widget, title))
            .highlight_symbol(symbol)
            .highlight_style(
                Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::ITALIC | Modifier::DIM),
            )
    };

    let worker_busy = {
        match state.worker_state {
            WorkerState::Idle => false,
            WorkerState::Fetching => {
                state.fetching_spinner.tick();
                true
            }
            WorkerState::Error(ref e) => {
                let error = Paragraph::new(e.to_string()).block(block(WidgetId::Error, "Error"));
                f.render_widget(error, f.size());
                return;
            }
        }
    };

    let hchunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(f.size());

    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(10), Constraint::Percentage(90)].as_ref())
        .split(hchunks[0]);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)].as_ref())
        .split(hchunks[1]);

    let city_input = Paragraph::new(state.user_city.as_ref())
        .block(block(WidgetId::Search, "Search"))
        .wrap(Wrap { trim: true });

    let symbol = if worker_busy {
        state.fetching_spinner.pattern().to_string() + " "
    } else {
        "> ".to_string()
    };

    let found_entries = list(
        WidgetId::Places,
        "Places",
        &symbol,
        state
            .places
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
    );

    let max_option_key_len = state.max_option_key_len();
    let options = list(
        WidgetId::Options,
        "Options",
        "* ",
        state
            .options
            .iter()
            .map(|(k, v)| {
                ListItem::new(format!(
                    "{}: {:spaces$}{}",
                    k,
                    " ",
                    v,
                    spaces = max_option_key_len - k.len()
                ))
            })
            .collect(),
    );

    let keybindings = Paragraph::new("TODO")
        .block(block(WidgetId::Keybindings, "Keybindigs"))
        .wrap(Wrap { trim: true });

    f.render_widget(city_input, left_chunks[0]);
    f.render_stateful_widget(found_entries, left_chunks[1], &mut state.places.state());

    if state.focus == WidgetId::Options {
        f.render_stateful_widget(options, right_chunks[0], &mut state.options.state());
    } else {
        f.render_widget(options, right_chunks[0]);
    }
    f.render_widget(keybindings, right_chunks[1]);
}

async fn handle_key_event(code: event::KeyCode, state_m: &Arc<Mutex<State>>) -> anyhow::Result<()> {
    use event::KeyCode;

    let mut state = state_m.lock().unwrap();

    if state.worker_busy() {
        return Ok(());
    }

    match state.focus {
        WidgetId::Search => match code {
            KeyCode::Enter => {
                if !state.user_city.is_empty() {
                    let user_city = state.user_city.clone();

                    state.fetch(
                        Arc::clone(&state_m),
                        async move { roads::search(&user_city).await.map_err(anyhow::Error::msg) },
                        |state, cities| {
                            state.places = WrappingList::new(cities);
                            state.focus = WidgetId::Places;
                            Ok(())
                        },
                    );
                }
            }
            code => {
                if edit_string(&mut state.user_city, code) {
                    state.places = WrappingList::new(vec![]);
                }
            }
        },
        WidgetId::Places => match code {
            KeyCode::Up | KeyCode::Char('k') => {
                state.places.up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.places.down();
            }
            KeyCode::Enter => {
                if let Some(place) = state.places.selected() {
                    let place: NominatimEntry = place.clone();

                    state.fetch(
                        Arc::clone(&state_m),
                        async move { roads::fetch_roads(&place).await.map_err(anyhow::Error::msg) },
                        move |state, paths| {
                            let w = state.option(State::WIDTH_OPTION).unwrap();
                            let h = state.option(State::HEIGHT_OPTION).unwrap();
                            let sw = state.option(State::STROKE_WIDTH_OPTION).unwrap();

                            dump_svg(&format!("{}.svg", &state.user_city), (w, h), sw, paths)?;
                            Ok(())
                        },
                    );
                }
            }
            _ => {}
        },
        WidgetId::Options => match code {
            KeyCode::Up | KeyCode::Char('k') => {
                state.options.up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.options.down();
            }
            _ => {
                if let Some(p) = state.options.selected_mut() {
                    let mut s = p.1.to_string();
                    edit_string(&mut s, code);
                    if let Ok(n) = s.parse::<f64>() {
                        p.1 = n;
                    }
                }
            }
        },
        WidgetId::Keybindings => {}
        WidgetId::Error => match code {
            KeyCode::Enter => {
                state.worker_state = WorkerState::Idle;
                state.focus = WidgetId::Search;
            }
            _ => {}
        },
    }

    Ok(())
}

fn edit_string(s: &mut String, code: event::KeyCode) -> bool {
    use event::KeyCode;

    match code {
        KeyCode::Backspace => {
            s.pop();
            true
        }
        KeyCode::Char(c) => {
            s.push(c);
            true
        }
        _ => false,
    }
}

fn dump_svg(
    path: &str,
    (w, h): (f64, f64),
    stroke_width: f64,
    mut paths: Vec<Vec<(f64, f64)>>,
) -> io::Result<()> {
    use std::f64::{INFINITY, NEG_INFINITY};

    let mut min_x = INFINITY;
    let mut min_y = INFINITY;
    let mut max_x = NEG_INFINITY;
    let mut max_y = NEG_INFINITY;

    for p in &mut paths {
        *p = roads::simplify::simplify(p);

        for (x, y) in p {
            *y *= -1.0;

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
<g stroke="black" stroke-width="{}" fill="none" >"#,
        (max_x - min_x) * sf,
        (max_y - min_y) * sf,
        stroke_width,
    )?;

    for p in paths {
        write!(f, r#"<polyline points=""#)?;
        for (x, y) in p {
            write!(f, "{:.2},{:.2} ", (x - min_x) * sf, (y - min_y) * sf)?;
        }
        writeln!(f, r#"" />"#)?;
    }

    writeln!(f, "</g>\n</svg>")?;

    Ok(())
}
