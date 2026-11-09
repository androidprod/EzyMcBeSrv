use anyhow::{Context, Result, bail};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ezymcbesrv::{Store, binary_path, default_data_dir, install_or_build, send_command, start};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    collections::HashMap,
    env,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Child,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

enum Input {
    None,
    New(String),
    Command(String),
}

struct App {
    store: Store,
    selected: usize,
    running: HashMap<String, Child>,
    stopping: Vec<String>,
    building: Option<String>,
    tx: Sender<(String, Result<()>)>,
    rx: Receiver<(String, Result<()>)>,
    input: Input,
    show_build_log: bool,
    message: String,
}

impl App {
    fn new(store: Store) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            store,
            selected: 0,
            running: HashMap::new(),
            stopping: Vec::new(),
            building: None,
            tx,
            rx,
            input: Input::None,
            show_build_log: false,
            message: "n: 新規サーバーを作成してください".into(),
        }
    }

    fn selected_name(&self) -> Option<String> {
        self.store
            .servers()
            .get(self.selected)
            .map(|s| s.name.clone())
    }

    fn build(&mut self, name: String) {
        if self.building.is_some() {
            self.message = "別のビルドが進行中です".into();
            return;
        }
        if self.running.contains_key(&name) {
            self.message = "稼働中はビルドできません".into();
            return;
        }
        install_or_build(self.store.directory(&name), self.tx.clone(), name.clone());
        self.building = Some(name.clone());
        self.message = format!("{name}: 取得・ビルド中（build.log を表示）");
    }

    fn tick(&mut self) {
        while let Ok((name, result)) = self.rx.try_recv() {
            self.building = None;
            self.message = match result {
                Ok(()) => format!("{name}: ビルド完了。s で起動できます"),
                Err(error) => format!("{name}: {error:#}"),
            };
        }
        let mut finished = Vec::new();
        for (name, child) in &mut self.running {
            match child.try_wait() {
                Ok(Some(status)) => {
                    finished.push((name.clone(), format!("{name}: 終了 ({status})")))
                }
                Err(error) => {
                    finished.push((name.clone(), format!("{name}: 状態確認に失敗: {error}")))
                }
                Ok(None) => {}
            }
        }
        for (name, message) in finished {
            self.running.remove(&name);
            self.stopping.retain(|item| item != &name);
            self.message = message;
        }
    }

    fn action(&mut self, key: KeyCode) -> Result<bool> {
        match &mut self.input {
            Input::New(text) | Input::Command(text) => {
                match key {
                    KeyCode::Esc => self.input = Input::None,
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char(c) if !c.is_control() && text.len() < 256 => text.push(c),
                    KeyCode::Enter => {
                        let mode = std::mem::replace(&mut self.input, Input::None);
                        match mode {
                            Input::New(name) => {
                                let name = name.trim();
                                match self.store.add(name) {
                                    Ok(()) => {
                                        self.selected = self.store.servers().len() - 1;
                                        self.build(name.to_owned());
                                    }
                                    Err(error) => self.message = format!("作成失敗: {error:#}"),
                                }
                            }
                            Input::Command(command) if !command.trim().is_empty() => {
                                if let Some(name) = self.selected_name() {
                                    let result = self
                                        .running
                                        .get_mut(&name)
                                        .context("not running")
                                        .and_then(|child| send_command(child, &command));
                                    self.message = match result {
                                        Ok(()) => format!("{name} に送信: {command}"),
                                        Err(error) => format!("送信失敗: {error:#}"),
                                    };
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            Input::None => {}
        }
        match key {
            KeyCode::Char('q') => {
                if !self.running.is_empty() || self.building.is_some() {
                    self.message =
                        "終了する前に x でサーバーを停止し、ビルドの完了を待ってください".into();
                } else {
                    return Ok(true);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.store.servers().len() {
                    self.selected += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('n') => self.input = Input::New(String::new()),
            KeyCode::Char('l') => self.show_build_log = !self.show_build_log,
            KeyCode::Char('b') => {
                if let Some(name) = self.selected_name() {
                    self.build(name);
                }
            }
            KeyCode::Char('s') => {
                if let Some(name) = self.selected_name() {
                    if self.running.contains_key(&name) {
                        self.message = "すでに起動しています".into();
                    } else if self.building.as_ref() == Some(&name) {
                        self.message = "ビルド中です".into();
                    } else {
                        match start(&self.store.directory(&name)) {
                            Ok(child) => {
                                self.running.insert(name.clone(), child);
                                self.message = format!("{name}: 起動しました");
                            }
                            Err(error) => self.message = format!("起動失敗: {error:#}"),
                        }
                    }
                }
            }
            KeyCode::Char('x') => {
                if let Some(name) = self.selected_name()
                    && let Some(child) = self.running.get_mut(&name)
                {
                    if self.stopping.contains(&name) {
                        self.message = "停止処理中です".into();
                    } else {
                        match send_command(child, "stop") {
                            Ok(()) => {
                                self.stopping.push(name.clone());
                                self.message = format!("{name}: stop を送信、保存・終了待機中");
                            }
                            Err(error) => self.message = format!("停止要求失敗: {error:#}"),
                        }
                    }
                }
            }
            KeyCode::Char('c') => {
                if self
                    .selected_name()
                    .is_some_and(|name| self.running.contains_key(&name))
                {
                    self.input = Input::Command(String::new());
                } else {
                    self.message = "起動中のサーバーを選択してください".into();
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn draw(&self, frame: &mut ratatui::Frame) {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(3),
                Constraint::Length(3),
            ])
            .split(frame.area());
        frame.render_widget(
            Paragraph::new(
                " EzyMcBeSrv   n:新規  ↑↓:選択  b:ビルド  s:起動  x:停止  c:コマンド  l:ログ切替  q:終了",
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Chorus server manager"),
            ),
            areas[0],
        );
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
            .split(areas[1]);
        let items: Vec<ListItem> = self
            .store
            .servers()
            .iter()
            .map(|server| {
                let status = if self.building.as_deref() == Some(&server.name) {
                    "ビルド中"
                } else if self.stopping.contains(&server.name) {
                    "停止中"
                } else if self.running.contains_key(&server.name) {
                    "稼働中"
                } else if binary_path(&self.store.directory(&server.name)).is_file() {
                    "停止"
                } else {
                    "未ビルド"
                };
                ListItem::new(format!("{}  [{status}]", server.name))
            })
            .collect();
        let mut state = ListState::default();
        if !items.is_empty() {
            state.select(Some(self.selected));
        }
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title("サーバー"))
                .highlight_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            columns[0],
            &mut state,
        );
        let name = self.selected_name();
        let path = name.as_ref().map(|name| self.store.directory(name));
        let build_log = self.show_build_log
            || name
                .as_ref()
                .is_some_and(|name| self.building.as_ref() == Some(name));
        let log_path = path
            .as_ref()
            .map(|dir| dir.join(if build_log { "build.log" } else { "server.log" }));
        let lines = log_path
            .as_ref()
            .map(|p| tail(p, columns[1].height.saturating_sub(2) as usize))
            .unwrap_or_default();
        let title = if build_log { "build.log" } else { "server.log" };
        frame.render_widget(
            Paragraph::new(lines.join("\n"))
                .block(Block::default().borders(Borders::ALL).title(title))
                .wrap(Wrap { trim: false }),
            columns[1],
        );
        let info = path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| self.store.root().display().to_string());
        frame.render_widget(
            Paragraph::new(info).block(Block::default().borders(Borders::ALL).title("保存先")),
            areas[2],
        );
        let (label, value) = match &self.input {
            Input::None => ("状態", self.message.as_str()),
            Input::New(text) => ("サーバー名 (Enter:作成 / Esc:キャンセル)", text.as_str()),
            Input::Command(text) => (
                "コンソールコマンド (Enter:送信 / Esc:キャンセル)",
                text.as_str(),
            ),
        };
        frame.render_widget(
            Paragraph::new(Line::raw(value))
                .block(Block::default().borders(Borders::ALL).title(label)),
            areas[3],
        );
    }
}

fn tail(path: &Path, count: usize) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = file.metadata().map(|meta| meta.len()) else {
        return Vec::new();
    };
    let offset = len.saturating_sub(128 * 1024);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.take(128 * 1024).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let text = if offset > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    text.lines()
        .rev()
        .take(count)
        .map(|line| line.chars().filter(|c| !c.is_control()).collect())
        .collect::<Vec<String>>()
        .into_iter()
        .rev()
        .collect()
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn data_dir_from_args() -> Result<PathBuf> {
    let mut args = env::args_os().skip(1);
    match args.next() {
        None => default_data_dir(),
        Some(flag) if flag == "--data-dir" => {
            let path = args.next().context("--data-dir requires a path")?;
            if args.next().is_some() {
                bail!("unexpected extra argument");
            }
            Ok(PathBuf::from(path))
        }
        Some(_) => bail!("usage: ezymcbesrv [--data-dir PATH]"),
    }
}

fn main() -> Result<()> {
    let store = Store::open(data_dir_from_args()?)?;
    let _guard = TerminalGuard::enter().context("terminal required")?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut app = App::new(store);
    loop {
        app.tick();
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && app.action(key.code)?
        {
            break;
        }
    }
    Ok(())
}
