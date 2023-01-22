use std::{
    any::Any,
    fmt::Display,
    fs,
    future::Future,
    io::{self, Write},
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};

use futures::StreamExt;
use tokio::runtime::Runtime;

use tui::{
    backend::{Backend, CrosstermBackend},
    text::{Span, Spans},
    Frame, Terminal,
};

use roads::{
    util::{DotsSpinner, WrappingList},
    NominatimEntry,
};

trait ParamValue: Display + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn cloned(&self) -> Box<dyn ParamValue>;
    fn from_str(&mut self, s: &str) -> bool;
}

impl<E, T: Clone + Display + Send + Sync + FromStr<Err = E> + 'static> ParamValue for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn cloned(&self) -> Box<dyn ParamValue> {
        Box::new(self.clone())
    }

    fn from_str(&mut self, s: &str) -> bool {
        match s.parse() {
            Ok(r) => {
                *self = r;
                true
            }
            Err(_) => false,
        }
    }
}

struct State {
    focus: WidgetId,
    user_city: String,
    places: WrappingList<NominatimEntry>,
    params: WrappingList<(&'static str, Box<dyn ParamValue>)>,
    worker_state: WorkerState,
    fetching_spinner: DotsSpinner,
    parm_edit_state: Option<ParmEditState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WidgetId {
    Places,
    Search,
    Options,
    Help,
    Error,
    ParamEdit,
}

enum WorkerState {
    Idle,
    Fetching,
    Error(anyhow::Error),
}

struct ParmEditState {
    buffer: String,
    value: Box<dyn ParamValue>,
    is_valid: bool,
}

impl ParmEditState {
    fn new(mut value: Box<dyn ParamValue>) -> Self {
        let buffer = value.to_string();
        let is_valid = value.from_str(&buffer);
        ParmEditState {
            buffer,
            value,
            is_valid,
        }
    }
}

impl State {
    const WIDTH_OPTION: &'static str = "Width";
    const HEIGHT_OPTION: &'static str = "Height";
    const STROKE_WIDTH_OPTION: &'static str = "Line width";
    const BACKGROUND_COLOR: &'static str = "Background color";
    const OPEN_OPTION: &'static str = "Open on save";

    fn new() -> Self {
        State {
            focus: WidgetId::Search,
            user_city: String::new(),
            places: WrappingList::new(vec![]),
            params: WrappingList::new(vec![
                (Self::WIDTH_OPTION, Box::new(1920.0)),
                (Self::HEIGHT_OPTION, Box::new(1080.0)),
                (Self::STROKE_WIDTH_OPTION, Box::new(0.3)),
                (Self::BACKGROUND_COLOR, Box::new("none".to_string())),
                (Self::OPEN_OPTION, Box::new(true)),
            ]),
            worker_state: WorkerState::Idle,
            fetching_spinner: DotsSpinner::new(),
            parm_edit_state: None,
        }
    }

    fn worker_busy(&self) -> bool {
        match self.worker_state {
            WorkerState::Fetching => true,
            WorkerState::Idle | WorkerState::Error(_) => false,
        }
    }

    fn max_option_key_len(&self) -> usize {
        self.params
            .iter()
            .map(|(k, _)| k.len())
            .max()
            .unwrap_or_default()
    }

    fn param<T: Any>(&self, key: &str) -> &T {
        for (k, v) in self.params.iter() {
            if k != &key {
                continue;
            }

            return v.as_any().downcast_ref::<T>().expect("invalid param type");
        }

        panic!("parameter {} not found", key)
    }

    fn set_current_param(&mut self, value: Box<dyn ParamValue>) {
        if let Some((_, v)) = self.params.selected_mut() {
            *v = value;
        }
    }

    fn fetch<T: Send + 'static>(
        &mut self,
        state: Arc<Mutex<Self>>,
        fut: impl Future<Output = anyhow::Result<T>> + Send + 'static,
        mut on_success: impl FnMut(&mut Self, T) -> anyhow::Result<()> + Send + 'static,
    ) {
        self.worker_state = WorkerState::Fetching;
        self.fetching_spinner = DotsSpinner::new();

        let _complete = tokio::task::spawn(async move {
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
    let mut reader = EventStream::new();
    let state = Arc::new(Mutex::new(State::new()));

    loop {
        terminal.draw(|f| {
            let mut state = state.lock().unwrap();
            draw(f, &mut state)
        })?;

        let ev = match tokio::time::timeout(Duration::from_millis(50), reader.next()).await {
            Err(_) => {
                // timeout expired
                continue;
            }
            Ok(ev) => ev,
        };

        let mut st = state.lock().unwrap();
        match ev {
            Some(Ok(event)) => {
                let KeyEvent {
                    code, modifiers, ..
                } = match event {
                    Event::Key(k) => k,
                    _ => continue,
                };

                if st.focus != WidgetId::ParamEdit {
                    if code == KeyCode::Esc
                        || (code, modifiers) == (KeyCode::Char('c'), KeyModifiers::CONTROL)
                    {
                        break;
                    }

                    if code == KeyCode::Tab || code == KeyCode::BackTab {
                        let tab_order = [
                            WidgetId::Search,
                            WidgetId::Places,
                            WidgetId::Options,
                            WidgetId::Help,
                        ];
                        let current = tab_order.iter().position(|w| w == &st.focus).unwrap();
                        let next = current
                            + if code == KeyCode::Tab {
                                1
                            } else {
                                tab_order.len() - 1
                            };

                        st.focus = tab_order[next % tab_order.len()];
                        continue;
                    }
                }

                handle_key_event(code, &mut st, &state).await?;
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

    let runtime = Runtime::new().unwrap();
    let _ = runtime.block_on(main_loop(&mut terminal));

    terminal.clear()?;
    crossterm::terminal::disable_raw_mode()?;

    Ok(())
}

fn draw(f: &mut Frame<impl Backend>, state: &mut State) {
    use tui::{
        layout::{Constraint, Direction, Layout},
        style::{Color, Modifier, Style},
        widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
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
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)].as_ref())
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
            .params
            .iter()
            .map(|(k, v)| {
                let mut s = k.to_string();
                s += ": ";
                for _ in 0..max_option_key_len - k.len() {
                    s.push(' ');
                }
                s += &v.to_string();

                ListItem::new(s)
            })
            .collect(),
    );

    let help = Paragraph::new(
        r#"Simple TUI to render the roads of a given place into an svg file.

To start off, search a place by editing the Search line edit, hit enter and select the desired place to render.

Use the arrow keys or jk to move up and down and <TAB> to switch section.

Hit <Enter> on an option to edit it.

Esc or Ctrl-C to quit.
"#,
    )
    .block(block(WidgetId::Help, "Help"))
    .wrap(Wrap { trim: true });

    f.render_widget(city_input, left_chunks[0]);
    f.render_stateful_widget(found_entries, left_chunks[1], &mut state.places.state());

    if state.focus == WidgetId::Options {
        f.render_stateful_widget(options, right_chunks[0], &mut state.params.state());
    } else {
        f.render_widget(options, right_chunks[0]);
    }
    f.render_widget(help, right_chunks[1]);

    if state.focus == WidgetId::ParamEdit {
        let edit_state = state.parm_edit_state.as_ref().unwrap();

        if let Some((param, _)) = state.params.selected() {
            let parm_edit = Paragraph::new(edit_state.buffer.as_ref())
                .block(block(WidgetId::ParamEdit, param))
                .wrap(Wrap { trim: true })
                .style(if edit_state.is_valid {
                    Style::default()
                } else {
                    Style::default().bg(Color::LightRed)
                });

            let hcentered = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(30),
                    Constraint::Percentage(40),
                    Constraint::Percentage(30),
                ])
                .split(f.size());
            let vcentered = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(30),
                    Constraint::Max(3),
                    Constraint::Percentage(30),
                ])
                .split(hcentered[1]);

            f.render_widget(Clear, vcentered[1]);
            f.render_widget(parm_edit, vcentered[1]);
        }
    }
}

async fn handle_key_event(
    code: KeyCode,
    state: &mut State,
    state_m: &Arc<Mutex<State>>,
) -> anyhow::Result<()> {
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
                            let w = *state.param::<f64>(State::WIDTH_OPTION);
                            let h = *state.param::<f64>(State::HEIGHT_OPTION);
                            let sw = *state.param::<f64>(State::STROKE_WIDTH_OPTION);
                            let background = state.param::<String>(State::BACKGROUND_COLOR);

                            let path = format!("{}.svg", &state.user_city);
                            dump_svg(&path, (w, h), sw, background, paths)?;

                            let open_on_save = *state.param::<bool>(State::OPEN_OPTION);
                            if open_on_save {
                                opener::open(&path)?;
                            }

                            Ok(())
                        },
                    );
                }
            }
            _ => {}
        },
        WidgetId::Options => match code {
            KeyCode::Up | KeyCode::Char('k') => {
                state.params.up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.params.down();
            }
            KeyCode::Enter => {
                if let Some((_param, value)) = state.params.selected() {
                    state.parm_edit_state = Some(ParmEditState::new(value.cloned()));
                    state.focus = WidgetId::ParamEdit;
                }
            }
            _ => {}
        },
        WidgetId::ParamEdit => match code {
            KeyCode::Enter => {
                if state.parm_edit_state.as_ref().unwrap().is_valid {
                    let mut edit_state = None;
                    std::mem::swap(&mut edit_state, &mut state.parm_edit_state);

                    state.set_current_param(edit_state.unwrap().value);
                    state.focus = WidgetId::Options;
                }
            }
            KeyCode::Esc => {
                state.parm_edit_state = None;
                state.focus = WidgetId::Options;
            }
            _ => {
                let edit_state = state.parm_edit_state.as_mut().unwrap();
                edit_string(&mut edit_state.buffer, code);

                edit_state.is_valid = edit_state.value.from_str(&edit_state.buffer);
            }
        },
        WidgetId::Help => {}
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

fn edit_string(s: &mut String, code: KeyCode) -> bool {
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
    background_color: &str,
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
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.2} {h:.2}">
<rect x="0" y="0" width="{w:.2}" height="{h:.2}" fill="{background}" stroke="none"/>
<g stroke="black" stroke-width="{}" fill="none" >"#,
        stroke_width,
        w = (max_x - min_x) * sf,
        h = (max_y - min_y) * sf,
        background = background_color,
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
