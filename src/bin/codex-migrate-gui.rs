#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use codex_migrate::model::{
    DiagnosticReport, ImportOptions, ImportPlan, MergeAction, SourceCatalog, SourceProject,
    SourceSession,
};
use codex_migrate::operations::{
    self, ExportSummary, ImportSummary, TransactionSummary, VerificationReport,
};
use codex_migrate::path_mapper::{map_explicit, normalize};
use codex_migrate::settings::{self, AppSettings, LanguagePreference};
use eframe::egui::{
    self, Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Layout, Margin,
    Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextStyle, TextWrapMode, Vec2, WidgetText,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

const ACCENT: Color32 = Color32::from_rgb(31, 111, 104);
const TEXT: Color32 = Color32::from_rgb(39, 45, 48);
const MUTED: Color32 = Color32::from_rgb(91, 101, 105);
const BORDER: Color32 = Color32::from_rgb(220, 222, 218);
const SURFACE: Color32 = Color32::from_rgb(255, 255, 253);
const BACKGROUND: Color32 = Color32::from_rgb(246, 247, 244);
const WARNING: Color32 = Color32::from_rgb(159, 101, 38);
const DANGER: Color32 = Color32::from_rgb(174, 65, 58);
const OPTICAL_TEXT_OFFSET_Y: f32 = 3.0;

#[derive(Clone, Copy)]
enum LineIcon {
    Brand,
    Import,
    Export,
    Recovery,
    Logs,
    Folder,
    ChevronDown,
    ChevronRight,
    Check,
    Warning,
    Repair,
    Html,
    Settings,
}

fn main() -> eframe::Result<()> {
    let icon = application_icon();
    eframe::run_native(
        "Codex Migrate",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1240.0, 820.0])
                .with_min_inner_size([980.0, 680.0])
                .with_icon(icon),
            ..Default::default()
        },
        Box::new(|context| Ok(Box::new(MigrationApp::new(context)))),
    )
}

fn application_icon() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../../assets/icons/codex-migrate-512.png"))
        .expect("embedded app icon must be a valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Import,
    Export,
    Repair,
    Html,
    Recovery,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportStep {
    Choose,
    Select,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckState {
    None,
    Partial,
    All,
}

struct UiSession {
    source: SourceSession,
    selected: bool,
}

struct UiProject {
    original_cwd: String,
    target_path: String,
    history_only: bool,
    expanded: bool,
    sessions: Vec<UiSession>,
}

struct ConfirmationSpec<'a> {
    title: &'a str,
    message: &'a str,
    details: &'a [&'a str],
    action: &'a str,
    destructive: bool,
    chinese: bool,
}

#[derive(Clone)]
struct CompletionNotice {
    title: String,
    message: String,
    detail: String,
}

impl UiProject {
    fn state(&self) -> CheckState {
        let selected = self
            .sessions
            .iter()
            .filter(|session| session.selected)
            .count();
        match selected {
            0 => CheckState::None,
            value if value == self.sessions.len() => CheckState::All,
            _ => CheckState::Partial,
        }
    }

    fn set_selected(&mut self, selected: bool) {
        for session in &mut self.sessions {
            session.selected = selected;
        }
    }

    fn selected_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|session| session.selected)
            .count()
    }

    fn is_ready(&self) -> bool {
        self.selected_count() == 0
            || self.history_only
            || (!self.target_path.is_empty() && Path::new(&self.target_path).is_dir())
    }
}

enum TaskEvent {
    Progress(String),
    Complete(Box<TaskResult>),
}

enum TaskResult {
    Scan(Result<SourceCatalog, String>),
    RepairScan(Result<SourceCatalog, String>),
    HtmlScan(Result<SourceCatalog, String>),
    Plan(Result<ImportPlan, String>),
    Import(Result<ImportSummary, String>),
    Rebind(Result<ImportSummary, String>),
    HtmlExport(Result<codex_migrate::html_export::HtmlExportSummary, String>),
    Export(Result<ExportSummary, String>),
    Diagnose(Result<DiagnosticReport, String>),
    Verify(Result<VerificationReport, String>),
    Transactions(Result<Vec<TransactionSummary>, String>),
    Rollback(Result<String, String>),
    DeleteTransactions(Result<usize, String>),
}

struct MigrationApp {
    page: Page,
    step: ImportStep,
    source_folder: String,
    target_folder: String,
    export_source: String,
    export_parent: String,
    catalog: Option<SourceCatalog>,
    projects: Vec<UiProject>,
    repair_projects: Vec<UiProject>,
    html_projects: Vec<UiProject>,
    html_export_folder: String,
    show_all_repair_projects: bool,
    parent_source: String,
    parent_target: String,
    plan: Option<ImportPlan>,
    diagnostic: Option<DiagnosticReport>,
    transactions: Vec<TransactionSummary>,
    selected_transaction: Option<String>,
    selected_transactions: BTreeSet<String>,
    busy: bool,
    receiver: Option<Receiver<TaskEvent>>,
    status: String,
    logs: Vec<String>,
    show_logs: bool,
    error: Option<String>,
    success: Option<String>,
    import_completion: Option<ImportSummary>,
    completion_notice: Option<CompletionNotice>,
    confirm_import: bool,
    confirm_rollback: bool,
    confirm_delete_transactions: bool,
    confirm_rebind: bool,
    settings: AppSettings,
    system_chinese: bool,
}

impl MigrationApp {
    fn new(context: &eframe::CreationContext<'_>) -> Self {
        configure_style(&context.egui_ctx);
        install_system_font(&context.egui_ctx);
        let default_home = codex_migrate::discovery::discover(None)
            .map(|environment| environment.codex_home)
            .unwrap_or_else(|_| PathBuf::from(".codex"))
            .to_string_lossy()
            .into_owned();
        let settings = AppSettings::load();
        let system_chinese = settings::system_is_chinese();
        let chinese = matches!(settings.language, LanguagePreference::Chinese)
            || matches!(settings.language, LanguagePreference::System) && system_chinese;
        Self {
            page: Page::Import,
            step: ImportStep::Choose,
            source_folder: String::new(),
            target_folder: default_home.clone(),
            export_source: default_home,
            export_parent: dirs::download_dir()
                .or_else(dirs::desktop_dir)
                .unwrap_or_else(|| PathBuf::from("."))
                .to_string_lossy()
                .into_owned(),
            catalog: None,
            projects: Vec::new(),
            repair_projects: Vec::new(),
            html_projects: Vec::new(),
            html_export_folder: String::new(),
            show_all_repair_projects: false,
            parent_source: String::new(),
            parent_target: String::new(),
            plan: None,
            diagnostic: None,
            transactions: Vec::new(),
            selected_transaction: None,
            selected_transactions: BTreeSet::new(),
            busy: false,
            receiver: None,
            status: tr(
                chinese,
                "请选择旧设备 Codex 文件夹开始迁移",
                "Choose the old Codex folder to begin",
            )
            .to_owned(),
            logs: Vec::new(),
            show_logs: false,
            error: None,
            success: None,
            import_completion: None,
            completion_notice: None,
            confirm_import: false,
            confirm_rollback: false,
            confirm_delete_transactions: false,
            confirm_rebind: false,
            settings,
            system_chinese,
        }
    }

    fn chinese(&self) -> bool {
        match self.settings.language {
            LanguagePreference::System => self.system_chinese,
            LanguagePreference::Chinese => true,
            LanguagePreference::English => false,
        }
    }

    fn start_task(&mut self, task: impl FnOnce(Sender<TaskEvent>) + Send + 'static) {
        if self.busy {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.busy = true;
        self.error = None;
        self.success = None;
        self.receiver = Some(receiver);
        std::thread::spawn(move || task(sender));
    }

    fn poll_tasks(&mut self, context: &egui::Context) {
        let mut events = Vec::new();
        if let Some(receiver) = &self.receiver {
            while let Ok(event) = receiver.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            match event {
                TaskEvent::Progress(message) => {
                    self.status = message.clone();
                    self.logs.push(message);
                }
                TaskEvent::Complete(result) => {
                    self.busy = false;
                    self.receiver = None;
                    self.apply_result(*result);
                }
            }
        }
        if self.busy {
            context.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn apply_result(&mut self, result: TaskResult) {
        let zh = self.chinese();
        match result {
            TaskResult::Scan(result) => match result {
                Ok(catalog) => {
                    self.projects = catalog
                        .projects
                        .iter()
                        .map(project_to_ui)
                        .collect::<Vec<_>>();
                    self.parent_source = common_parent(
                        self.projects
                            .iter()
                            .map(|project| project.original_cwd.as_str()),
                    )
                    .unwrap_or_default();
                    self.status = format!(
                        "{} {} {}、{} {}",
                        tr(zh, "已读取", "Loaded"),
                        self.projects.len(),
                        tr(zh, "个项目", "projects"),
                        catalog.thread_count,
                        tr(zh, "个会话", "sessions")
                    );
                    self.success = Some(self.status.clone());
                    self.catalog = Some(catalog);
                    self.plan = None;
                    self.step = ImportStep::Select;
                }
                Err(error) => self.fail(error),
            },
            TaskResult::RepairScan(result) => match result {
                Ok(catalog) => {
                    self.repair_projects = catalog.projects.iter().map(project_to_ui).collect();
                    for project in &mut self.repair_projects {
                        project.set_selected(project_has_path_problem(project));
                    }
                    self.parent_source = common_parent(
                        self.repair_projects
                            .iter()
                            .map(|project| project.original_cwd.as_str()),
                    )
                    .unwrap_or_default();
                    self.status = format!(
                        "{} {}，{} {}",
                        tr(zh, "已读取", "Loaded"),
                        self.repair_projects.len(),
                        tr(zh, "个项目", "projects"),
                        catalog.thread_count
                    );
                }
                Err(error) => self.fail(error),
            },
            TaskResult::HtmlScan(result) => match result {
                Ok(catalog) => {
                    self.html_projects = catalog.projects.iter().map(project_to_ui).collect();
                    set_projects_selected(&mut self.html_projects, false);
                    self.status = format!(
                        "{} {} {}",
                        tr(zh, "已读取", "Loaded"),
                        catalog.thread_count,
                        tr(zh, "个本机会话", "local sessions")
                    );
                }
                Err(error) => self.fail(error),
            },
            TaskResult::Plan(result) => match result {
                Ok(plan) => {
                    let conflicts = plan
                        .threads
                        .iter()
                        .filter(|thread| thread.action == MergeAction::Conflict)
                        .map(|thread| thread.thread.id.clone())
                        .collect::<BTreeSet<_>>();
                    if !conflicts.is_empty() {
                        for project in &mut self.projects {
                            for session in &mut project.sessions {
                                if conflicts.contains(&session.source.thread.id) {
                                    session.selected = false;
                                }
                            }
                        }
                        self.error = Some(format!(
                            "{} {}",
                            conflicts.len(),
                            tr(
                                zh,
                                "个分叉冲突会话已自动取消选择，请重新预览",
                                "divergent sessions were deselected; preview again"
                            )
                        ));
                        self.plan = None;
                        self.step = ImportStep::Select;
                    } else {
                        self.status = format!(
                            "{}: {} {}",
                            tr(zh, "预览完成", "Preview ready"),
                            plan.threads.len(),
                            tr(zh, "个会话", "sessions")
                        );
                        self.success = Some(self.status.clone());
                        self.plan = Some(plan);
                        self.step = ImportStep::Preview;
                    }
                }
                Err(error) => self.fail(error),
            },
            TaskResult::Import(result) => match result {
                Ok(summary) => {
                    self.status = format!(
                        "{}: {} {}, {} {}",
                        tr(zh, "导入完成", "Import completed"),
                        summary.imported,
                        tr(zh, "个新增", "new"),
                        summary.refreshed,
                        tr(zh, "个索引已刷新", "indexes refreshed")
                    );
                    self.success = Some(format!(
                        "{}. {}: {}",
                        self.status,
                        tr(zh, "回滚事务", "Rollback transaction"),
                        summary.transaction_id
                    ));
                    self.selected_transaction = Some(summary.transaction_id.clone());
                    self.import_completion = Some(summary);
                    self.refresh_transactions();
                }
                Err(error) => self.fail(error),
            },
            TaskResult::Rebind(result) => match result {
                Ok(summary) => {
                    self.status = format!(
                        "{}：{}",
                        tr(zh, "路径修复完成", "Path repair completed"),
                        summary.refreshed + summary.imported
                    );
                    self.import_completion = Some(summary);
                }
                Err(error) => self.fail(error),
            },
            TaskResult::HtmlExport(result) => match result {
                Ok(summary) => {
                    self.status = format!(
                        "{} {} {}",
                        tr(zh, "已导出", "Exported"),
                        summary.exported,
                        tr(zh, "个 HTML 会话", "HTML sessions")
                    );
                    self.success = Some(self.status.clone());
                    let detail = summary
                        .files
                        .first()
                        .and_then(|path| Path::new(path).parent())
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.completion_notice = Some(CompletionNotice {
                        title: tr(zh, "HTML 导出完成", "HTML export completed").to_owned(),
                        message: format!(
                            "{} {} {}",
                            tr(zh, "已导出", "Exported"),
                            summary.exported,
                            tr(zh, "个会话", "sessions")
                        ),
                        detail,
                    });
                }
                Err(error) => self.fail(error),
            },
            TaskResult::Export(result) => match result {
                Ok(summary) => {
                    self.status = format!(
                        "{} {} {}",
                        tr(zh, "已导出", "Exported"),
                        summary.thread_count,
                        tr(zh, "个会话", "sessions")
                    );
                    self.success = Some(format!("{}：{}", self.status, summary.output));
                    self.completion_notice = Some(CompletionNotice {
                        title: tr(zh, "备份导出完成", "Backup export completed").to_owned(),
                        message: format!(
                            "{} {} {}",
                            tr(zh, "已导出", "Exported"),
                            summary.thread_count,
                            tr(zh, "个会话", "sessions")
                        ),
                        detail: summary.output,
                    });
                }
                Err(error) => self.fail(error),
            },
            TaskResult::Diagnose(result) => match result {
                Ok(report) => {
                    self.status = if report.issues.is_empty() {
                        tr(
                            zh,
                            "诊断完成，未发现迁移问题",
                            "Diagnostics completed; no migration issues found",
                        )
                        .to_owned()
                    } else {
                        format!(
                            "{} {} {}",
                            tr(zh, "诊断完成，发现", "Diagnostics found"),
                            report.issues.len(),
                            tr(zh, "个问题", "issues")
                        )
                    };
                    self.diagnostic = Some(report);
                }
                Err(error) => self.fail(error),
            },
            TaskResult::Verify(result) => match result {
                Ok(report) => {
                    self.status = tr(zh, "完整验证通过", "Full verification passed").to_owned();
                    self.success = Some(self.status.clone());
                    self.diagnostic = Some(report.migration_check);
                }
                Err(error) => self.fail(error),
            },
            TaskResult::Transactions(result) => match result {
                Ok(transactions) => {
                    if self.selected_transaction.as_ref().is_none_or(|selected| {
                        !transactions
                            .iter()
                            .any(|transaction| &transaction.id == selected)
                    }) {
                        self.selected_transaction =
                            transactions.first().map(|value| value.id.clone());
                    }
                    self.selected_transactions = transactions
                        .iter()
                        .map(|transaction| transaction.id.clone())
                        .collect();
                    self.transactions = transactions;
                }
                Err(error) => self.fail(error),
            },
            TaskResult::Rollback(result) => match result {
                Ok(id) => {
                    self.status = format!(
                        "{} {}",
                        tr(zh, "已回滚事务", "Rolled back transaction"),
                        short_id(&id)
                    );
                    self.success = Some(self.status.clone());
                    self.selected_transaction = None;
                    self.refresh_transactions();
                }
                Err(error) => self.fail(error),
            },
            TaskResult::DeleteTransactions(result) => match result {
                Ok(deleted) => {
                    self.status = format!(
                        "{} {} {}",
                        tr(zh, "已删除", "Deleted"),
                        deleted,
                        tr(zh, "个回滚备份", "rollback backups")
                    );
                    self.completion_notice = Some(CompletionNotice {
                        title: tr(zh, "回滚数据已删除", "Rollback data deleted").to_owned(),
                        message: self.status.clone(),
                        detail: String::new(),
                    });
                    self.selected_transactions.clear();
                    self.refresh_transactions();
                }
                Err(error) => self.fail(error),
            },
        }
    }

    fn fail(&mut self, error: String) {
        self.status = tr(self.chinese(), "操作失败", "Operation failed").to_owned();
        self.logs.push(error.clone());
        self.error = Some(error);
    }

    fn scan(&mut self) {
        let source = PathBuf::from(self.source_folder.trim());
        let zh = self.chinese();
        self.start_task(move |sender| {
            let _ = sender.send(TaskEvent::Progress(
                tr(
                    zh,
                    "正在读取来源 Codex 文件夹…",
                    "Reading source Codex folder…",
                )
                .to_owned(),
            ));
            let result = operations::scan_source(&source).map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(TaskResult::Scan(result))));
        });
    }

    fn scan_repair(&mut self) {
        let target = optional_path(&self.target_folder);
        self.start_task(move |sender| {
            let result = operations::scan_local(target.as_deref()).map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(TaskResult::RepairScan(
                result,
            ))));
        });
    }

    fn scan_html(&mut self) {
        let target = optional_path(&self.target_folder);
        self.start_task(move |sender| {
            let result = operations::scan_local(target.as_deref()).map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(TaskResult::HtmlScan(result))));
        });
    }

    fn rebind(&mut self) {
        let target = optional_path(&self.target_folder);
        let options = options_from_projects(&self.repair_projects);
        self.start_task(move |sender| {
            let progress_sender = sender.clone();
            let result = operations::rebind_existing(target.as_deref(), &options, move |message| {
                let _ = progress_sender.send(TaskEvent::Progress(message));
            })
            .map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(TaskResult::Rebind(result))));
        });
    }

    fn export_selected_html(&mut self) {
        let target = optional_path(&self.target_folder);
        let selected = selected_ids(&self.html_projects);
        let output = optional_path(&self.html_export_folder);
        self.start_task(move |sender| {
            let result = operations::export_html(target.as_deref(), &selected, output.as_deref())
                .map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(TaskResult::HtmlExport(
                result,
            ))));
        });
    }

    fn apply_parent_mapping(&mut self) {
        apply_parent_mapping_to(
            &mut self.repair_projects,
            &self.parent_source,
            &self.parent_target,
        );
    }

    fn apply_import_parent_mapping(&mut self) {
        apply_parent_mapping_to(&mut self.projects, &self.parent_source, &self.parent_target);
        self.plan = None;
    }

    fn preview(&mut self) {
        let source = PathBuf::from(self.source_folder.trim());
        let target = optional_path(&self.target_folder);
        let options = self.import_options();
        let zh = self.chinese();
        self.start_task(move |sender| {
            let _ = sender.send(TaskEvent::Progress(
                tr(zh, "正在计算导入计划…", "Calculating import plan…").to_owned(),
            ));
            let result = operations::plan_directory_import(&source, target.as_deref(), &options)
                .map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(TaskResult::Plan(result))));
        });
    }

    fn import(&mut self) {
        self.import_completion = None;
        let source = PathBuf::from(self.source_folder.trim());
        let target = optional_path(&self.target_folder);
        let options = self.import_options();
        self.start_task(move |sender| {
            let progress_sender = sender.clone();
            let result = operations::import_directory(
                &source,
                target.as_deref(),
                &options,
                move |message| {
                    let _ = progress_sender.send(TaskEvent::Progress(message));
                },
            )
            .map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(TaskResult::Import(result))));
        });
    }

    fn export(&mut self) {
        let source = PathBuf::from(self.export_source.trim());
        let parent = PathBuf::from(self.export_parent.trim());
        self.start_task(move |sender| {
            let progress_sender = sender.clone();
            let result = operations::export_directory(&source, &parent, move |message| {
                let _ = progress_sender.send(TaskEvent::Progress(message));
            })
            .map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(TaskResult::Export(result))));
        });
    }

    fn diagnose(&mut self, verify: bool) {
        let target = optional_path(&self.target_folder);
        self.start_task(move |sender| {
            let result = if verify {
                TaskResult::Verify(operations::verify(target.as_deref()).map_err(display_error))
            } else {
                TaskResult::Diagnose(operations::diagnose(target.as_deref()).map_err(display_error))
            };
            let _ = sender.send(TaskEvent::Complete(Box::new(result)));
        });
    }

    fn refresh_transactions(&mut self) {
        let target = optional_path(&self.target_folder);
        self.start_task(move |sender| {
            let result = operations::list_transactions(target.as_deref()).map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(TaskResult::Transactions(
                result,
            ))));
        });
    }

    fn rollback(&mut self) {
        let Some(id) = self.selected_transaction.clone() else {
            self.error = Some(
                tr(
                    self.chinese(),
                    "请先选择一个回滚事务",
                    "Select a rollback transaction first",
                )
                .to_owned(),
            );
            return;
        };
        let target = optional_path(&self.target_folder);
        self.start_task(move |sender| {
            let result = operations::rollback(target.as_deref(), &id)
                .map(|_| id)
                .map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(TaskResult::Rollback(result))));
        });
    }

    fn delete_selected_transactions(&mut self) {
        if self.selected_transactions.is_empty() {
            self.error = Some(
                tr(
                    self.chinese(),
                    "请至少选择一个要删除的回滚事务",
                    "Select at least one rollback transaction to delete",
                )
                .to_owned(),
            );
            return;
        }
        let target = optional_path(&self.target_folder);
        let ids = self
            .selected_transactions
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        self.start_task(move |sender| {
            let result =
                operations::delete_transactions(target.as_deref(), &ids).map_err(display_error);
            let _ = sender.send(TaskEvent::Complete(Box::new(
                TaskResult::DeleteTransactions(result),
            )));
        });
    }

    fn import_options(&self) -> ImportOptions {
        let mut selected_thread_ids = BTreeSet::new();
        let mut mappings = BTreeMap::new();
        let mut history_only_projects = BTreeSet::new();
        for project in &self.projects {
            let cwd = normalize(&project.original_cwd);
            for session in &project.sessions {
                if session.selected {
                    selected_thread_ids.insert(session.source.thread.id.clone());
                }
            }
            if project.history_only {
                history_only_projects.insert(cwd);
            } else if !project.target_path.is_empty() {
                mappings.insert(cwd, project.target_path.clone());
            }
        }
        ImportOptions {
            selected_thread_ids,
            mappings,
            history_only_projects,
        }
    }

    fn selected_count(&self) -> usize {
        self.projects.iter().map(UiProject::selected_count).sum()
    }

    fn project_count(&self) -> usize {
        self.projects
            .iter()
            .filter(|project| project.selected_count() > 0)
            .count()
    }

    fn pending_mapping_count(&self) -> usize {
        self.projects
            .iter()
            .filter(|project| !project.is_ready())
            .count()
    }

    fn all_state(&self) -> CheckState {
        let selected = self.selected_count();
        let total = self
            .projects
            .iter()
            .map(|project| project.sessions.len())
            .sum::<usize>();
        match selected {
            0 => CheckState::None,
            value if value == total && total > 0 => CheckState::All,
            _ => CheckState::Partial,
        }
    }

    fn set_all(&mut self, selected: bool) {
        for project in &mut self.projects {
            project.set_selected(selected);
        }
        self.plan = None;
        if self.step == ImportStep::Preview {
            self.step = ImportStep::Select;
        }
    }

    fn sidebar(&mut self, context: &egui::Context) {
        let zh = self.chinese();
        egui::SidePanel::left("sidebar")
            .exact_width(210.0)
            .frame(egui::Frame::new().fill(Color32::from_rgb(250, 250, 248)))
            .show(context, |ui| {
                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    line_icon(ui, LineIcon::Brand, 22.0, ACCENT);
                    ui.add_space(4.0);
                    optical_label(
                        ui,
                        RichText::new("Codex Migrate")
                            .size(17.0)
                            .strong()
                            .color(TEXT),
                    );
                });
                ui.add_space(30.0);
                nav_button(
                    ui,
                    &mut self.page,
                    Page::Import,
                    LineIcon::Import,
                    tr(zh, "导入会话", "Import sessions"),
                );
                nav_button(
                    ui,
                    &mut self.page,
                    Page::Export,
                    LineIcon::Export,
                    tr(zh, "导出备份", "Backup"),
                );
                nav_button(
                    ui,
                    &mut self.page,
                    Page::Repair,
                    LineIcon::Repair,
                    tr(zh, "路径修复", "Repair paths"),
                );
                nav_button(
                    ui,
                    &mut self.page,
                    Page::Html,
                    LineIcon::Html,
                    tr(zh, "导出 HTML", "Export HTML"),
                );
                nav_button(
                    ui,
                    &mut self.page,
                    Page::Recovery,
                    LineIcon::Recovery,
                    tr(zh, "诊断与回滚", "Diagnostics"),
                );
                nav_button(
                    ui,
                    &mut self.page,
                    Page::Settings,
                    LineIcon::Settings,
                    tr(zh, "设置", "Settings"),
                );
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(
                            RichText::new(format!(
                                "v{} · {}",
                                env!("CARGO_PKG_VERSION"),
                                tr(zh, "V1 开发版", "V1")
                            ))
                            .small()
                            .color(MUTED),
                        );
                    });
                });
            });
    }

    fn top_status(&mut self, context: &egui::Context) {
        let zh = self.chinese();
        egui::TopBottomPanel::top("top_status")
            .frame(
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::symmetric(24, 13)),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    if self.busy {
                        ui.spinner();
                    } else {
                        line_icon(ui, LineIcon::Check, 17.0, ACCENT);
                    }
                    optical_label(ui, RichText::new(&self.status).size(14.0).color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_text_button(
                            ui,
                            LineIcon::Logs,
                            if self.show_logs {
                                tr(zh, "收起日志", "Hide logs")
                            } else {
                                tr(zh, "查看日志", "View logs")
                            },
                        )
                        .clicked()
                        {
                            self.show_logs = !self.show_logs;
                        }
                        ui.label(
                            RichText::new(format!(
                                "{}: {}",
                                tr(zh, "本机 Codex", "Local Codex"),
                                self.target_folder
                            ))
                            .small()
                            .color(MUTED),
                        );
                    });
                });
                if let Some(error) = &self.error {
                    ui.add_space(6.0);
                    status_message(ui, LineIcon::Warning, DANGER, error);
                }
                if let Some(success) = self
                    .success
                    .as_ref()
                    .filter(|success| success.as_str() != self.status.as_str())
                {
                    ui.add_space(6.0);
                    status_message(ui, LineIcon::Check, ACCENT, success);
                }
            });
    }

    fn log_drawer(&mut self, context: &egui::Context) {
        if !self.show_logs {
            return;
        }
        let zh = self.chinese();
        egui::TopBottomPanel::bottom("logs")
            .resizable(true)
            .default_height(150.0)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    line_icon(ui, LineIcon::Logs, 16.0, MUTED);
                    optical_label(
                        ui,
                        RichText::new(tr(zh, "详细日志", "Detailed logs"))
                            .size(14.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if secondary_button(ui, None, tr(zh, "清空", "Clear")).clicked() {
                            self.logs.clear();
                        }
                    });
                });
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.logs {
                            ui.label(RichText::new(line).monospace().small());
                        }
                    });
            });
    }

    fn import_page(&mut self, ui: &mut egui::Ui) {
        let zh = self.chinese();
        page_title(
            ui,
            tr(zh, "选择要导入的项目与会话", "Choose projects and sessions"),
            tr(
                zh,
                "读取旧设备 Codex 文件夹，映射项目目录，并按需选择历史会话。",
                "Read an old Codex folder, map project directories, and select sessions.",
            ),
        );
        stepper(ui, self.step, zh);
        ui.add_space(16.0);
        match self.step {
            ImportStep::Choose => self.choose_step(ui),
            ImportStep::Select => self.select_step(ui),
            ImportStep::Preview => self.preview_step(ui),
        }
    }

    fn choose_step(&mut self, ui: &mut egui::Ui) {
        let zh = self.chinese();
        card(ui, |ui| {
            ui.strong(tr(zh, "来源与目标", "Source and target"));
            ui.add_space(14.0);
            folder_row(
                ui,
                tr(zh, "旧设备 Codex 文件夹", "Old Codex folder"),
                &mut self.source_folder,
                tr(zh, "选择来源文件夹", "Choose source"),
            );
            ui.add_space(14.0);
            folder_row(
                ui,
                tr(zh, "本机 Codex 文件夹", "Local Codex folder"),
                &mut self.target_folder,
                tr(zh, "更改目标文件夹", "Change target"),
            );
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(tr(
                        zh,
                        "支持精简 Codex/ 或完整 .codex/ 文件夹",
                        "Supports compact Codex/ and full .codex/ folders",
                    ))
                    .small()
                    .color(MUTED),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_action(
                        ui,
                        tr(zh, "读取项目与会话", "Load projects and sessions"),
                        !self.busy && !self.source_folder.is_empty(),
                    )
                    .clicked()
                    {
                        self.scan();
                    }
                });
            });
        });
    }

    fn select_step(&mut self, ui: &mut egui::Ui) {
        let zh = self.chinese();
        let selected = self.selected_count();
        let pending = self.pending_mapping_count();
        let all_state = self.all_state();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(tr(zh, "父目录映射", "Parent mapping"))
                    .size(13.0)
                    .strong(),
            );
            ui.text_edit_singleline(&mut self.parent_source);
            ui.label("→");
            ui.text_edit_singleline(&mut self.parent_target);
            if icon_only_button(
                ui,
                LineIcon::Folder,
                tr(zh, "选择新父目录", "Choose parent"),
            )
            .clicked()
            {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.parent_target = path.to_string_lossy().into_owned();
                }
            }
            if secondary_button(ui, None, tr(zh, "自动匹配", "Auto match")).clicked() {
                self.apply_import_parent_mapping();
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            badge(
                ui,
                &format!("{} {}", self.projects.len(), tr(zh, "个项目", "projects")),
                false,
            );
            badge(
                ui,
                &format!(
                    "{} {}",
                    self.projects
                        .iter()
                        .map(|project| project.sessions.len())
                        .sum::<usize>(),
                    tr(zh, "个会话", "sessions")
                ),
                false,
            );
            badge(
                ui,
                &format!("{} {}", tr(zh, "已选择", "Selected"), selected),
                true,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if selection_control(ui, all_state, tr(zh, "全选", "Select all")).clicked() {
                    self.set_all(all_state != CheckState::All);
                }
            });
        });
        ui.add_space(10.0);

        let available = ui.available_rect_before_wrap();
        let action_height = 62.0;
        let gap = 10.0;
        let list_height = (available.height() - action_height - gap).max(120.0);
        let list_rect =
            Rect::from_min_size(available.min, Vec2::new(available.width(), list_height));
        let action_rect = Rect::from_min_size(
            Pos2::new(available.min.x, list_rect.max.y + gap),
            Vec2::new(available.width(), action_height),
        );

        ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("project_session_list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for project_index in 0..self.projects.len() {
                        self.project_card(ui, project_index);
                        ui.add_space(10.0);
                    }
                });
        });

        ui.scope_builder(egui::UiBuilder::new().max_rect(action_rect), |ui| {
            sticky_action_bar(ui, |ui| {
                if selection_control(
                    ui,
                    all_state,
                    tr(zh, "全选项目与会话", "Select all projects and sessions"),
                )
                .clicked()
                {
                    self.set_all(all_state != CheckState::All);
                }
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{} {} {}、{} {} · {} {}",
                        tr(zh, "已选", "Selected"),
                        self.project_count(),
                        tr(zh, "个项目", "projects"),
                        selected,
                        tr(zh, "个会话", "sessions"),
                        pending,
                        tr(zh, "个路径待处理", "paths pending")
                    ))
                    .size(13.0)
                    .color(if pending > 0 { WARNING } else { MUTED }),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_action(
                        ui,
                        &format!("{} ({})", tr(zh, "预览导入", "Preview import"), selected),
                        selected > 0 && pending == 0 && !self.busy,
                    )
                    .clicked()
                    {
                        self.preview();
                    }
                    if secondary_button(ui, None, tr(zh, "重新选择目录", "Choose folders again"))
                        .clicked()
                    {
                        self.step = ImportStep::Choose;
                    }
                });
            });
        });
        ui.allocate_rect(available, Sense::hover());
    }

    fn project_card(&mut self, ui: &mut egui::Ui, index: usize) {
        let zh = self.chinese();
        let project = &mut self.projects[index];
        let state = project.state();
        let selected = project.selected_count();
        let total = project.sessions.len();
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(9.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Frame::new()
                    .fill(if state == CheckState::None {
                        SURFACE
                    } else {
                        Color32::from_rgb(241, 248, 246)
                    })
                    .corner_radius(9.0)
                    .inner_margin(Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if selection_control(ui, state, "").clicked() {
                                project.set_selected(state != CheckState::All);
                                self.plan = None;
                            }
                            if icon_only_button(
                                ui,
                                if project.expanded {
                                    LineIcon::ChevronDown
                                } else {
                                    LineIcon::ChevronRight
                                },
                                tr(zh, "展开或折叠项目", "Expand or collapse project"),
                            )
                            .clicked()
                            {
                                project.expanded = !project.expanded;
                            }
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(tr(zh, "旧设备项目", "Old project"))
                                        .size(12.0)
                                        .color(MUTED),
                                );
                                ui.label(
                                    RichText::new(&project.original_cwd)
                                        .size(15.0)
                                        .strong()
                                        .color(TEXT),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "{} {} · {} {}",
                                        total,
                                        tr(zh, "个会话", "sessions"),
                                        tr(zh, "已选择", "selected"),
                                        selected
                                    ))
                                    .size(12.0)
                                    .color(MUTED),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icon_text_button(
                                    ui,
                                    LineIcon::Folder,
                                    tr(zh, "选择文件夹", "Choose folder"),
                                )
                                .clicked()
                                {
                                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                        project.target_path = path.to_string_lossy().into_owned();
                                        project.history_only = false;
                                        self.plan = None;
                                    }
                                }
                            });
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_space(56.0);
                            line_icon(ui, LineIcon::Folder, 15.0, MUTED);
                            ui.label(
                                RichText::new(tr(zh, "本机项目", "Local project"))
                                    .size(12.0)
                                    .color(MUTED),
                            );
                            ui.label(if project.history_only {
                                RichText::new(tr(zh, "仅恢复历史", "History only"))
                                    .size(13.0)
                                    .color(ACCENT)
                            } else if project.target_path.is_empty() {
                                RichText::new(tr(zh, "尚未映射", "Not mapped"))
                                    .size(13.0)
                                    .color(WARNING)
                            } else {
                                RichText::new(&project.target_path).size(13.0).color(TEXT)
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let mut history = project.history_only;
                                if bool_control(
                                    ui,
                                    &mut history,
                                    tr(zh, "仅恢复历史", "History only"),
                                )
                                .on_hover_text(tr(
                                    zh,
                                    "不绑定实际项目目录，仅恢复可查看的会话历史",
                                    "Restore viewable history without binding a real project folder",
                                ))
                                    .changed()
                                {
                                    project.history_only = history;
                                    self.plan = None;
                                }
                            });
                        });
                        if selected > 0 && !project.is_ready() {
                            ui.add_space(8.0);
                            status_message(
                                ui,
                                LineIcon::Warning,
                                WARNING,
                                tr(
                                    zh,
                                    "请选择本机项目文件夹，或设为仅恢复历史",
                                    "Choose a local project folder or enable history-only mode",
                                ),
                            );
                        }
                    });
                if project.expanded {
                    ui.add_space(4.0);
                    for session in &mut project.sessions {
                        ui.horizontal(|ui| {
                            ui.add_space(56.0);
                            let mut selected = session.selected;
                            if bool_control(ui, &mut selected, "").changed() {
                                session.selected = selected;
                                self.plan = None;
                            }
                            ui.vertical(|ui| {
                                ui.label(if session.source.thread.title.is_empty() {
                                    RichText::new(tr(zh, "未命名会话", "Untitled session"))
                                        .size(14.0)
                                        .strong()
                                        .color(TEXT)
                                } else {
                                    RichText::new(&session.source.thread.title)
                                        .size(14.0)
                                        .strong()
                                        .color(TEXT)
                                });
                                ui.label(
                                    RichText::new(format!(
                                        "{}  ·  {}",
                                        format_date(session.source.thread.updated_at),
                                        if session.source.thread.archived {
                                            tr(zh, "已归档", "Archived")
                                        } else {
                                            tr(zh, "活动会话", "Active")
                                        }
                                    ))
                                    .size(12.0)
                                    .color(MUTED),
                                );
                            });
                        });
                        ui.add_space(7.0);
                    }
                    ui.add_space(6.0);
                }
            });
    }

    fn preview_step(&mut self, ui: &mut egui::Ui) {
        let zh = self.chinese();
        let Some(plan) = self.plan.clone() else {
            self.step = ImportStep::Select;
            return;
        };
        let import_count = plan
            .threads
            .iter()
            .filter(|thread| {
                matches!(
                    thread.action,
                    MergeAction::Import | MergeAction::ReplaceWithLonger
                )
            })
            .count();
        let skip_count = plan.threads.len() - import_count;
        ui.horizontal(|ui| {
            summary_metric(ui, tr(zh, "选择会话", "Selected"), plan.threads.len());
            summary_metric(ui, tr(zh, "将导入", "Import"), import_count);
            summary_metric(ui, tr(zh, "将跳过", "Skip"), skip_count);
            summary_metric(ui, tr(zh, "冲突", "Conflicts"), plan.conflicts);
        });
        ui.add_space(12.0);

        let available = ui.available_rect_before_wrap();
        let action_height = 62.0;
        let gap = 10.0;
        let list_height = (available.height() - action_height - gap).max(120.0);
        let list_rect =
            Rect::from_min_size(available.min, Vec2::new(available.width(), list_height));
        let action_rect = Rect::from_min_size(
            Pos2::new(available.min.x, list_rect.max.y + gap),
            Vec2::new(available.width(), action_height),
        );

        ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("import_preview_list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(tr(
                            zh,
                            "导入到以下本机项目",
                            "Import into these local projects",
                        ))
                        .size(15.0)
                        .strong()
                        .color(TEXT),
                    );
                    ui.add_space(10.0);
                    let mut projects = BTreeMap::<&str, Vec<_>>::new();
                    for thread in &plan.threads {
                        projects
                            .entry(thread.mapped_cwd.as_str())
                            .or_default()
                            .push(thread);
                    }
                    for (mapped_cwd, threads) in projects {
                        egui::Frame::new()
                            .fill(SURFACE)
                            .stroke(Stroke::new(1.0, BORDER))
                            .corner_radius(10.0)
                            .show(ui, |ui| {
                                egui::Frame::new()
                                    .fill(Color32::from_rgb(241, 248, 246))
                                    .corner_radius(10.0)
                                    .inner_margin(Margin::symmetric(16, 12))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            line_icon(ui, LineIcon::Folder, 17.0, ACCENT);
                                            optical_label(
                                                ui,
                                                RichText::new(tr(zh, "本机项目", "Local project"))
                                                    .size(12.0)
                                                    .color(MUTED),
                                            );
                                            optical_label(
                                                ui,
                                                RichText::new(mapped_cwd)
                                                    .size(14.0)
                                                    .strong()
                                                    .color(TEXT),
                                            );
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    badge(
                                                        ui,
                                                        &format!(
                                                            "{} {}",
                                                            threads.len(),
                                                            tr(zh, "个会话", "sessions")
                                                        ),
                                                        true,
                                                    );
                                                },
                                            );
                                        });
                                    });
                                ui.add_space(7.0);
                                for thread in threads {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.add_space(18.0);
                                        action_badge(ui, &thread.action, zh);
                                        ui.label(
                                            RichText::new(if thread.thread.title.is_empty() {
                                                &thread.thread.id
                                            } else {
                                                &thread.thread.title
                                            })
                                            .size(13.0)
                                            .color(TEXT),
                                        );
                                    });
                                    ui.horizontal_wrapped(|ui| {
                                        ui.add_space(76.0);
                                        ui.label(
                                            RichText::new(&thread.reason).size(12.0).color(MUTED),
                                        );
                                    });
                                    ui.add_space(7.0);
                                }
                                ui.add_space(3.0);
                            });
                        ui.add_space(10.0);
                    }
                });
        });

        ui.scope_builder(egui::UiBuilder::new().max_rect(action_rect), |ui| {
            sticky_action_bar(ui, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_action(
                        ui,
                        &format!(
                            "{} ({})",
                            tr(zh, "确认导入", "Confirm import"),
                            plan.threads.len()
                        ),
                        !self.busy,
                    )
                    .clicked()
                    {
                        self.confirm_import = true;
                    }
                    if secondary_button(ui, None, tr(zh, "返回修改", "Back")).clicked() {
                        self.step = ImportStep::Select;
                        self.plan = None;
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        status_message(
                            ui,
                            LineIcon::Warning,
                            MUTED,
                            tr(
                                zh,
                                "导入前请关闭 Codex 应用和所有 Codex CLI 会话",
                                "Quit Codex and all Codex CLI sessions before importing",
                            ),
                        );
                    });
                });
            });
        });
        ui.allocate_rect(available, Sense::hover());
    }

    fn export_page(&mut self, ui: &mut egui::Ui) {
        let zh = self.chinese();
        page_title(
            ui,
            tr(zh, "导出精简 Codex 备份", "Export compact Codex backup"),
            tr(
                zh,
                "在所选父目录中创建结构兼容的 Codex/ 文件夹，不包含认证和设备数据。",
                "Create a compatible Codex/ folder without authentication or device data.",
            ),
        );
        card(ui, |ui| {
            folder_row(
                ui,
                tr(zh, "本机 Codex 文件夹", "Local Codex folder"),
                &mut self.export_source,
                tr(zh, "选择来源", "Choose source"),
            );
            ui.add_space(16.0);
            folder_row(
                ui,
                tr(zh, "导出到父目录", "Export parent folder"),
                &mut self.export_parent,
                tr(zh, "选择位置", "Choose location"),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(tr(
                        zh,
                        "将创建：所选目录/Codex/",
                        "Creates: selected folder/Codex/",
                    ))
                    .small()
                    .color(MUTED),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_action(ui, tr(zh, "导出备份", "Export backup"), !self.busy).clicked()
                    {
                        self.export();
                    }
                });
            });
        });
    }

    fn repair_page(&mut self, ui: &mut egui::Ui) {
        let zh = self.chinese();
        page_title(
            ui,
            tr(zh, "修复现有会话路径", "Repair existing session paths"),
            tr(
                zh,
                "用于直接复制或同步 .codex 后，将旧设备项目路径重新绑定到本机目录。",
                "Rebind project paths after copying or syncing a .codex folder between devices.",
            ),
        );
        card(ui, |ui| {
            folder_row(
                ui,
                tr(zh, "本机 Codex 文件夹", "Local Codex folder"),
                &mut self.target_folder,
                tr(zh, "选择目录", "Choose folder"),
            );
            ui.add_space(12.0);
            if primary_action(
                ui,
                tr(zh, "读取现有会话", "Load existing sessions"),
                !self.busy,
            )
            .clicked()
            {
                self.scan_repair();
            }
        });
        if self.repair_projects.is_empty() {
            return;
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let mut show_all = self.show_all_repair_projects;
            if bool_control(ui, &mut show_all, tr(zh, "全部项目", "All projects")).changed() {
                self.show_all_repair_projects = show_all;
            }
            ui.label(
                RichText::new(if self.show_all_repair_projects {
                    tr(zh, "正在显示全部项目", "Showing all projects")
                } else {
                    tr(
                        zh,
                        "仅显示旧路径在本机不存在的项目",
                        "Showing only projects whose old path is missing",
                    )
                })
                .size(12.0)
                .color(MUTED),
            );
        });
        ui.add_space(8.0);
        card(ui, |ui| {
            ui.label(
                RichText::new(tr(zh, "父目录批量映射", "Parent-folder mapping"))
                    .size(15.0)
                    .strong()
                    .color(TEXT),
            );
            ui.label(
                RichText::new(tr(
                    zh,
                    "例如将 /Volumes/Data/Project 下的全部项目自动匹配到 ~/Documents/Project。",
                    "Example: map every project under /Volumes/Data/Project to ~/Documents/Project.",
                ))
                .size(12.0)
                .color(MUTED),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(tr(zh, "旧父目录", "Old parent"));
                ui.text_edit_singleline(&mut self.parent_source);
                ui.label(tr(zh, "新父目录", "New parent"));
                ui.text_edit_singleline(&mut self.parent_target);
                if icon_text_button(
                    ui,
                    LineIcon::Folder,
                    tr(zh, "选择新父目录", "Choose new parent"),
                )
                .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.parent_target = path.to_string_lossy().into_owned();
                    }
                }
                if secondary_button(ui, None, tr(zh, "自动匹配", "Auto match")).clicked() {
                    self.apply_parent_mapping();
                }
            });
        });
        ui.add_space(12.0);
        let show_all = self.show_all_repair_projects;
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 74.0).max(180.0))
            .show(ui, |ui| {
                let mut visible = 0;
                for project in self
                    .repair_projects
                    .iter_mut()
                    .filter(|project| show_all || project_has_path_problem(project))
                {
                    visible += 1;
                    repair_project_row(ui, project, zh);
                    ui.add_space(8.0);
                }
                if visible == 0 {
                    ui.label(
                        RichText::new(tr(
                            zh,
                            "没有发现路径有问题的项目。",
                            "No projects with path problems were found.",
                        ))
                        .color(MUTED),
                    );
                }
            });
        sticky_action_bar(ui, |ui| {
            let selected = selected_ids(&self.repair_projects).len();
            let pending = self
                .repair_projects
                .iter()
                .filter(|project| project.selected_count() > 0 && !project.is_ready())
                .count();
            ui.label(format!(
                "{} {} · {} {}",
                selected,
                tr(zh, "个会话", "sessions"),
                pending,
                tr(zh, "个路径待处理", "paths pending")
            ));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_action(
                    ui,
                    tr(zh, "应用路径修复", "Apply path repair"),
                    selected > 0 && pending == 0 && !self.busy,
                )
                .clicked()
                {
                    self.confirm_rebind = true;
                }
            });
        });
    }

    fn html_page(&mut self, ui: &mut egui::Ui) {
        let zh = self.chinese();
        page_title(
            ui,
            tr(zh, "导出会话为 HTML", "Export sessions as HTML"),
            tr(
                zh,
                "每个会话会导出到对应项目下的 Codex_sessions 文件夹。",
                "Each session is exported to a Codex_sessions folder inside its project.",
            ),
        );
        card(ui, |ui| {
            folder_row(
                ui,
                tr(zh, "本机 Codex 文件夹", "Local Codex folder"),
                &mut self.target_folder,
                tr(zh, "选择目录", "Choose folder"),
            );
            ui.add_space(12.0);
            if primary_action(
                ui,
                tr(zh, "读取本机会话", "Load local sessions"),
                !self.busy,
            )
            .clicked()
            {
                self.scan_html();
            }
            ui.add_space(14.0);
            folder_row(
                ui,
                tr(
                    zh,
                    "自定义导出位置（留空则导出到各项目/Codex_sessions）",
                    "Custom export folder (leave empty for each project/Codex_sessions)",
                ),
                &mut self.html_export_folder,
                tr(zh, "选择导出位置", "Choose export folder"),
            );
            if !self.html_export_folder.is_empty()
                && secondary_button(ui, None, tr(zh, "恢复默认位置", "Use default folders"))
                    .clicked()
            {
                self.html_export_folder.clear();
            }
        });
        if self.html_projects.is_empty() {
            return;
        }
        ui.add_space(12.0);
        let all_state = projects_selection_state(&self.html_projects);
        ui.horizontal(|ui| {
            if selection_control(ui, all_state, tr(zh, "全部选择", "Select all")).clicked() {
                set_projects_selected(&mut self.html_projects, all_state != CheckState::All);
            }
            ui.label(
                RichText::new(tr(
                    zh,
                    "默认不选择任何会话",
                    "No sessions are selected by default",
                ))
                .size(12.0)
                .color(MUTED),
            );
        });
        ui.add_space(8.0);

        let available = ui.available_rect_before_wrap();
        let action_height = 62.0;
        let gap = 10.0;
        let list_height = (available.height() - action_height - gap).max(120.0);
        let list_rect =
            Rect::from_min_size(available.min, Vec2::new(available.width(), list_height));
        let action_rect = Rect::from_min_size(
            Pos2::new(available.min.x, list_rect.max.y + gap),
            Vec2::new(available.width(), action_height),
        );

        ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("html_session_list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for project in &mut self.html_projects {
                        session_selection_card(ui, project, zh);
                        ui.add_space(8.0);
                    }
                });
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(action_rect), |ui| {
            sticky_action_bar(ui, |ui| {
                let selected = selected_ids(&self.html_projects).len();
                if selection_control(
                    ui,
                    projects_selection_state(&self.html_projects),
                    tr(zh, "全部选择", "Select all"),
                )
                .clicked()
                {
                    let select = projects_selection_state(&self.html_projects) != CheckState::All;
                    set_projects_selected(&mut self.html_projects, select);
                }
                ui.separator();
                ui.label(format!(
                    "{} {}",
                    selected,
                    tr(zh, "个会话已选择", "sessions selected")
                ));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_action(
                        ui,
                        tr(zh, "导出 HTML", "Export HTML"),
                        selected > 0 && !self.busy,
                    )
                    .clicked()
                    {
                        self.export_selected_html();
                    }
                });
            });
        });
        ui.allocate_rect(available, Sense::hover());
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        let zh = self.chinese();
        page_title(
            ui,
            tr(zh, "设置", "Settings"),
            tr(
                zh,
                "界面语言默认跟随电脑系统，也可以手动切换。",
                "The interface follows the system language by default and can be overridden.",
            ),
        );
        card(ui, |ui| {
            ui.label(
                RichText::new(tr(zh, "界面语言", "Interface language"))
                    .size(15.0)
                    .strong(),
            );
            ui.add_space(10.0);
            let mut changed = false;
            changed |= ui
                .radio_value(
                    &mut self.settings.language,
                    LanguagePreference::System,
                    tr(zh, "跟随系统", "Follow system"),
                )
                .changed();
            changed |= ui
                .radio_value(
                    &mut self.settings.language,
                    LanguagePreference::Chinese,
                    "简体中文",
                )
                .changed();
            changed |= ui
                .radio_value(
                    &mut self.settings.language,
                    LanguagePreference::English,
                    "English",
                )
                .changed();
            if changed {
                if let Err(error) = self.settings.save() {
                    self.error = Some(error.to_string());
                } else {
                    self.status = tr(
                        self.chinese(),
                        "语言设置已保存",
                        "Language preference saved",
                    )
                    .to_owned();
                }
            }
        });
    }

    fn recovery_page(&mut self, ui: &mut egui::Ui) {
        let zh = self.chinese();
        page_title(
            ui,
            tr(zh, "诊断与回滚", "Diagnostics and rollback"),
            tr(
                zh,
                "验证本机 Codex 会话索引，或恢复一次导入前的安全快照。",
                "Verify local session indexes or restore a pre-import snapshot.",
            ),
        );
        card(ui, |ui| {
            folder_row(
                ui,
                tr(zh, "本机 Codex 文件夹", "Local Codex folder"),
                &mut self.target_folder,
                tr(zh, "选择目录", "Choose folder"),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if secondary_button(
                    ui,
                    Some(LineIcon::Recovery),
                    tr(zh, "快速诊断", "Quick diagnostics"),
                )
                .clicked()
                {
                    self.diagnose(false);
                }
                if primary_action(ui, tr(zh, "完整验证", "Full verification"), !self.busy).clicked()
                {
                    self.diagnose(true);
                }
                if secondary_button(
                    ui,
                    Some(LineIcon::Recovery),
                    tr(zh, "刷新回滚事务", "Refresh transactions"),
                )
                .clicked()
                {
                    self.refresh_transactions();
                }
            });
        });
        if let Some(report) = &self.diagnostic {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                summary_metric(ui, tr(zh, "活动会话", "Active"), report.active_rollouts);
                summary_metric(ui, tr(zh, "归档会话", "Archived"), report.archived_rollouts);
                summary_metric(
                    ui,
                    tr(zh, "数据库会话", "Database"),
                    report.database_threads,
                );
                summary_metric(ui, tr(zh, "问题", "Issues"), report.issues.len());
            });
        }
        ui.add_space(14.0);
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(tr(zh, "导入事务", "Import transactions"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let state = transaction_selection_state(
                        &self.transactions,
                        &self.selected_transactions,
                    );
                    if selection_control(ui, state, tr(zh, "全选", "Select all")).clicked() {
                        if state == CheckState::All {
                            self.selected_transactions.clear();
                        } else {
                            self.selected_transactions = self
                                .transactions
                                .iter()
                                .map(|transaction| transaction.id.clone())
                                .collect();
                        }
                    }
                });
            });
            ui.add_space(8.0);
            for transaction in &self.transactions {
                let selected = self.selected_transaction.as_deref() == Some(&transaction.id);
                let mut checked = self.selected_transactions.contains(&transaction.id);
                let label = format!(
                    "{}  ·  {}  ·  {}",
                    transaction.created_at,
                    short_id(&transaction.id),
                    Path::new(&transaction.source_codex_home)
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or(&transaction.source_codex_home)
                );
                ui.horizontal(|ui| {
                    if bool_control(ui, &mut checked, "").changed() {
                        if checked {
                            self.selected_transactions.insert(transaction.id.clone());
                        } else {
                            self.selected_transactions.remove(&transaction.id);
                        }
                    }
                    if ui.selectable_label(selected, label).clicked() {
                        self.selected_transaction = Some(transaction.id.clone());
                    }
                });
            }
            if self.transactions.is_empty() {
                ui.label(
                    RichText::new(tr(zh, "暂无迁移事务", "No migration transactions")).color(MUTED),
                );
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} {} {}",
                        tr(zh, "已选择", "Selected"),
                        self.selected_transactions.len(),
                        tr(zh, "个备份", "backups")
                    ))
                    .size(12.0)
                    .color(MUTED),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !self.selected_transactions.is_empty() && !self.busy,
                            egui::Button::new(
                                RichText::new(tr(zh, "删除所选备份", "Delete selected backups"))
                                    .color(DANGER),
                            ),
                        )
                        .clicked()
                    {
                        self.confirm_delete_transactions = true;
                    }
                    if ui
                        .add_enabled(
                            self.selected_transaction.is_some() && !self.busy,
                            egui::Button::new(
                                RichText::new(tr(
                                    zh,
                                    "回滚当前事务",
                                    "Rollback current transaction",
                                ))
                                .color(DANGER),
                            ),
                        )
                        .clicked()
                    {
                        self.confirm_rollback = true;
                    }
                });
            });
        });
    }

    fn confirmation_windows(&mut self, context: &egui::Context) {
        let zh = self.chinese();
        let mut import_confirmed = false;
        if self.confirm_import {
            confirmation(
                context,
                ConfirmationSpec {
                    title: tr(zh, "确认导入", "Confirm import"),
                    message: tr(
                        zh,
                        "即将把所选会话写入本机 Codex。开始前请关闭 Codex 应用和所有 Codex CLI 会话。",
                        "Selected sessions will be written to local Codex. Quit Codex and all CLI sessions first.",
                    ),
                    details: &[
                        tr(zh, "先创建 SQLite 回滚快照", "Create a SQLite rollback snapshot"),
                        tr(zh, "复制并注册所选会话", "Copy and register selected sessions"),
                        tr(zh, "任何步骤失败都会自动恢复", "Automatically restore on failure"),
                    ],
                    action: tr(zh, "确认导入", "Confirm import"),
                    destructive: false,
                    chinese: zh,
                },
                &mut self.confirm_import,
                || import_confirmed = true,
            );
        }
        if import_confirmed {
            self.import();
        }

        let mut rollback_confirmed = false;
        if self.confirm_rollback {
            confirmation(
                context,
                ConfirmationSpec {
                    title: tr(zh, "确认回滚", "Confirm rollback"),
                    message: tr(
                        zh,
                        "该操作会撤销所选迁移事务，并恢复到导入开始前的状态。",
                        "This restores the state from before the selected import transaction.",
                    ),
                    details: &[
                        tr(
                            zh,
                            "恢复导入前的数据库快照",
                            "Restore the database snapshot",
                        ),
                        tr(
                            zh,
                            "删除本次新增的会话文件",
                            "Remove newly added session files",
                        ),
                    ],
                    action: tr(zh, "确认回滚", "Confirm rollback"),
                    destructive: true,
                    chinese: zh,
                },
                &mut self.confirm_rollback,
                || rollback_confirmed = true,
            );
        }
        if rollback_confirmed {
            self.rollback();
        }

        let mut delete_confirmed = false;
        if self.confirm_delete_transactions {
            let count = self.selected_transactions.len();
            let message = format!(
                "{} {} {}{}",
                tr(zh, "即将永久删除所选的", "Permanently delete the selected"),
                count,
                tr(zh, "个回滚备份", "rollback backups"),
                tr(
                    zh,
                    "。删除后无法再使用这些快照回滚。",
                    ". They cannot be used again."
                )
            );
            let detail = tr(
                zh,
                "只删除 migration_transactions 中的备份，不修改当前会话",
                "Only backup data is removed; current sessions are unchanged",
            );
            confirmation(
                context,
                ConfirmationSpec {
                    title: tr(zh, "确认删除回滚数据", "Delete rollback data"),
                    message: &message,
                    details: &[detail],
                    action: tr(zh, "永久删除", "Delete permanently"),
                    destructive: true,
                    chinese: zh,
                },
                &mut self.confirm_delete_transactions,
                || delete_confirmed = true,
            );
        }
        if delete_confirmed {
            self.delete_selected_transactions();
        }

        let mut rebind_confirmed = false;
        if self.confirm_rebind {
            confirmation(
                context,
                ConfirmationSpec {
                    title: tr(zh, "确认修复路径", "Confirm path repair"),
                    message: tr(
                        zh,
                        "将修改所选会话的项目路径、会话索引和 SQLite 元数据。请先完全关闭 Codex。",
                        "Selected session paths, session index entries, and SQLite metadata will be updated. Quit Codex first.",
                    ),
                    details: &[
                        tr(zh, "创建可回滚备份", "Create a rollback backup"),
                        tr(zh, "只修改结构化路径，不修改对话正文", "Change structured paths only"),
                    ],
                    action: tr(zh, "应用修复", "Apply repair"),
                    destructive: false,
                    chinese: zh,
                },
                &mut self.confirm_rebind,
                || rebind_confirmed = true,
            );
        }
        if rebind_confirmed {
            self.rebind();
        }

        if let Some(summary) = self.import_completion.clone() {
            let mut close = false;
            import_success_modal(context, &summary, &mut close, zh);
            if close {
                self.import_completion = None;
            }
        }
        if let Some(notice) = self.completion_notice.clone() {
            let mut close = false;
            completion_notice_modal(context, &notice, &mut close, zh);
            if close {
                self.completion_notice = None;
            }
        }
    }

    fn current_page(&mut self, ui: &mut egui::Ui) {
        match self.page {
            Page::Import => self.import_page(ui),
            Page::Export => self.export_page(ui),
            Page::Repair => self.repair_page(ui),
            Page::Html => self.html_page(ui),
            Page::Recovery => self.recovery_page(ui),
            Page::Settings => self.settings_page(ui),
        }
    }
}

impl eframe::App for MigrationApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_tasks(context);
        self.sidebar(context);
        self.top_status(context);
        self.log_drawer(context);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(Margin::symmetric(28, 22)),
            )
            .show(context, |ui| {
                let content_width = ui.available_width().min(1100.0);
                let content_size = Vec2::new(content_width, ui.available_height());
                let fixed_layout = (self.page == Page::Import
                    && matches!(self.step, ImportStep::Select | ImportStep::Preview))
                    || matches!(self.page, Page::Repair | Page::Html);
                if fixed_layout {
                    ui.allocate_ui_with_layout(content_size, Layout::top_down(Align::Min), |ui| {
                        self.current_page(ui);
                    });
                } else {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(content_width);
                            self.current_page(ui);
                            ui.add_space(20.0);
                        });
                }
            });
        self.confirmation_windows(context);
    }
}

fn project_to_ui(project: &SourceProject) -> UiProject {
    UiProject {
        original_cwd: project.original_cwd.clone(),
        target_path: project.suggested_target.clone().unwrap_or_default(),
        history_only: false,
        expanded: true,
        sessions: project
            .sessions
            .iter()
            .cloned()
            .map(|source| UiSession {
                source,
                selected: true,
            })
            .collect(),
    }
}

fn selected_ids(projects: &[UiProject]) -> BTreeSet<String> {
    projects
        .iter()
        .flat_map(|project| project.sessions.iter())
        .filter(|session| session.selected)
        .map(|session| session.source.thread.id.clone())
        .collect()
}

fn projects_selection_state(projects: &[UiProject]) -> CheckState {
    let selected = projects
        .iter()
        .map(UiProject::selected_count)
        .sum::<usize>();
    let total = projects
        .iter()
        .map(|project| project.sessions.len())
        .sum::<usize>();
    match selected {
        0 => CheckState::None,
        value if value == total && total > 0 => CheckState::All,
        _ => CheckState::Partial,
    }
}

fn set_projects_selected(projects: &mut [UiProject], selected: bool) {
    for project in projects {
        project.set_selected(selected);
    }
}

fn transaction_selection_state(
    transactions: &[TransactionSummary],
    selected: &BTreeSet<String>,
) -> CheckState {
    let count = transactions
        .iter()
        .filter(|transaction| selected.contains(&transaction.id))
        .count();
    match count {
        0 => CheckState::None,
        value if value == transactions.len() && !transactions.is_empty() => CheckState::All,
        _ => CheckState::Partial,
    }
}

fn options_from_projects(projects: &[UiProject]) -> ImportOptions {
    let mut mappings = BTreeMap::new();
    for project in projects {
        if project.selected_count() > 0 && !project.target_path.is_empty() {
            mappings.insert(
                normalize(&project.original_cwd),
                project.target_path.clone(),
            );
        }
    }
    ImportOptions {
        selected_thread_ids: selected_ids(projects),
        mappings,
        history_only_projects: BTreeSet::new(),
    }
}

fn common_parent<'a>(paths: impl Iterator<Item = &'a str>) -> Option<String> {
    let paths = paths.map(normalize).collect::<Vec<_>>();
    let first = paths.first()?;
    let mut parts = first.split('/').collect::<Vec<_>>();
    for path in &paths[1..] {
        let other = path.split('/').collect::<Vec<_>>();
        let shared = parts
            .iter()
            .zip(other.iter())
            .take_while(|(left, right)| left == right)
            .count();
        parts.truncate(shared);
    }
    let joined = parts.join("/");
    Some(if first.starts_with('/') {
        format!("/{joined}").replace("//", "/")
    } else {
        joined
    })
}

fn apply_parent_mapping_to(projects: &mut [UiProject], source: &str, target: &str) {
    let source = normalize(source);
    let target = normalize(target);
    if source.is_empty() || target.is_empty() {
        return;
    }
    let mappings = [(source, target)].into_iter().collect::<BTreeMap<_, _>>();
    let platform = codex_migrate::discovery::current_platform();
    for project in projects {
        if let Some(mapped) = map_explicit(&project.original_cwd, &mappings, &platform) {
            project.target_path = if mapped.is_dir() {
                mapped.to_string_lossy().into_owned()
            } else {
                String::new()
            };
            project.history_only = false;
        }
    }
}

fn project_has_path_problem(project: &UiProject) -> bool {
    !Path::new(&project.original_cwd).is_dir()
}

fn repair_project_row(ui: &mut egui::Ui, project: &mut UiProject, zh: bool) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(9.0)
        .inner_margin(Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if selection_control(ui, project.state(), "").clicked() {
                    project.set_selected(project.state() != CheckState::All);
                }
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(&project.original_cwd)
                            .size(14.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new(if project.target_path.is_empty() {
                            tr(zh, "尚未匹配本机目录", "No local folder matched")
                        } else {
                            &project.target_path
                        })
                        .size(12.0)
                        .color(if project.target_path.is_empty() {
                            WARNING
                        } else {
                            MUTED
                        }),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_text_button(ui, LineIcon::Folder, tr(zh, "选择文件夹", "Choose folder"))
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            project.target_path = path.to_string_lossy().into_owned();
                        }
                    }
                });
            });
        });
}

fn session_selection_card(ui: &mut egui::Ui, project: &mut UiProject, zh: bool) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(9.0)
        .inner_margin(Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if selection_control(ui, project.state(), "").clicked() {
                    project.set_selected(project.state() != CheckState::All);
                }
                ui.add(
                    egui::Label::new(
                        RichText::new(&project.original_cwd)
                            .size(14.0)
                            .strong()
                            .color(TEXT),
                    )
                    .truncate(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(format!(
                        "{} {}",
                        project.selected_count(),
                        tr(zh, "个已选", "selected")
                    ));
                });
            });
            ui.add_space(7.0);
            for session in &mut project.sessions {
                ui.horizontal(|ui| {
                    ui.add_space(28.0);
                    let mut selected = session.selected;
                    if bool_control(ui, &mut selected, "").changed() {
                        session.selected = selected;
                    }
                    ui.add(
                        egui::Label::new(if session.source.thread.title.is_empty() {
                            &session.source.thread.id
                        } else {
                            &session.source.thread.title
                        })
                        .truncate(),
                    );
                });
            }
        });
}

fn tr<'a>(chinese: bool, chinese_text: &'a str, english_text: &'a str) -> &'a str {
    if chinese {
        chinese_text
    } else {
        english_text
    }
}

fn nav_button(ui: &mut egui::Ui, page: &mut Page, value: Page, icon: LineIcon, label: &str) {
    let selected = *page == value;
    let response = egui::Frame::new()
        .fill(if selected {
            Color32::from_rgb(232, 243, 240)
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_width(176.0);
            ui.horizontal(|ui| {
                line_icon(ui, icon, 17.0, if selected { ACCENT } else { MUTED });
                ui.add_space(6.0);
                optical_label(
                    ui,
                    RichText::new(label).size(14.0).strong().color(if selected {
                        TEXT
                    } else {
                        MUTED
                    }),
                );
            });
        })
        .response;
    if response.interact(egui::Sense::click()).clicked() {
        *page = value;
    }
}

fn page_title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(24.0).strong().color(TEXT));
    ui.add_space(4.0);
    ui.label(RichText::new(subtitle).size(13.0).color(MUTED));
    ui.add_space(16.0);
}

fn stepper(ui: &mut egui::Ui, active: ImportStep, zh: bool) {
    ui.horizontal(|ui| {
        step(
            ui,
            "1",
            tr(zh, "选择目录", "Choose folders"),
            active != ImportStep::Choose,
            active == ImportStep::Choose,
        );
        line(ui, active != ImportStep::Choose);
        step(
            ui,
            "2",
            tr(zh, "映射与选择", "Map and select"),
            active == ImportStep::Preview,
            active == ImportStep::Select,
        );
        line(ui, active == ImportStep::Preview);
        step(
            ui,
            "3",
            tr(zh, "确认导入", "Confirm import"),
            false,
            active == ImportStep::Preview,
        );
    });
}

fn step(ui: &mut egui::Ui, number: &str, label: &str, completed: bool, active: bool) {
    let color = if completed || active { ACCENT } else { MUTED };
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    ui.painter()
        .circle_stroke(rect.center(), 8.0, Stroke::new(1.5, color));
    if completed {
        paint_line_icon(ui.painter(), rect.shrink(4.0), LineIcon::Check, color);
    } else {
        ui.painter().text(
            rect.center() + Vec2::new(0.0, OPTICAL_TEXT_OFFSET_Y),
            Align2::CENTER_CENTER,
            number,
            FontId::proportional(12.0),
            color,
        );
    }
    optical_label(ui, RichText::new(label).size(13.0).strong().color(color));
}

fn line(ui: &mut egui::Ui, active: bool) {
    let (_, painter) = ui.allocate_painter(Vec2::new(48.0, 1.0), egui::Sense::hover());
    painter.line_segment(
        [
            painter.clip_rect().left_center(),
            painter.clip_rect().right_center(),
        ],
        Stroke::new(1.0, if active { ACCENT } else { BORDER }),
    );
}

fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(10.0)
        .inner_margin(20.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
}

fn folder_row(ui: &mut egui::Ui, label: &str, value: &mut String, button: &str) {
    ui.label(RichText::new(label).size(14.0).strong().color(TEXT));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 154.0).max(260.0);
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(width)
                .hint_text("Path / 路径"),
        );
        if icon_text_button(ui, LineIcon::Folder, button).clicked() {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                *value = path.to_string_lossy().into_owned();
            }
        }
    });
}

fn badge(ui: &mut egui::Ui, text: &str, active: bool) {
    egui::Frame::new()
        .fill(if active {
            Color32::from_rgb(228, 241, 238)
        } else {
            Color32::from_rgb(238, 240, 237)
        })
        .corner_radius(20.0)
        .inner_margin(Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .size(12.0)
                    .color(if active { ACCENT } else { MUTED }),
            );
        });
}

fn selection_control(ui: &mut egui::Ui, state: CheckState, label: &str) -> egui::Response {
    let response = egui::Frame::new()
        .inner_margin(Margin::symmetric(2, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                check_icon(ui, state);
                if !label.is_empty() {
                    ui.add_space(4.0);
                    optical_label(ui, RichText::new(label).size(13.0).color(TEXT));
                }
            });
        })
        .response
        .interact(Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn bool_control(ui: &mut egui::Ui, value: &mut bool, label: &str) -> egui::Response {
    let state = if *value {
        CheckState::All
    } else {
        CheckState::None
    };
    let mut response = selection_control(ui, state, label);
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response
}

fn optical_label(ui: &mut egui::Ui, text: RichText) -> egui::Response {
    let galley = WidgetText::from(text).into_galley(
        ui,
        Some(TextWrapMode::Extend),
        f32::INFINITY,
        TextStyle::Body,
    );
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(
        rect.min + Vec2::new(0.0, OPTICAL_TEXT_OFFSET_Y),
        galley,
        TEXT,
    );
    response
}

fn check_icon(ui: &mut egui::Ui, state: CheckState) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
    let box_rect = Rect::from_center_size(rect.center(), Vec2::splat(15.0));
    let active = state != CheckState::None;
    ui.painter().rect_filled(
        box_rect,
        3.5,
        if active { ACCENT } else { Color32::TRANSPARENT },
    );
    ui.painter().rect_stroke(
        box_rect,
        3.5,
        Stroke::new(1.4, if active { ACCENT } else { MUTED }),
        StrokeKind::Inside,
    );
    match state {
        CheckState::None => {}
        CheckState::Partial => {
            ui.painter().line_segment(
                [
                    Pos2::new(box_rect.left() + 3.5, box_rect.center().y),
                    Pos2::new(box_rect.right() - 3.5, box_rect.center().y),
                ],
                Stroke::new(1.7, Color32::WHITE),
            );
        }
        CheckState::All => {
            paint_line_icon(
                ui.painter(),
                box_rect.shrink(3.0),
                LineIcon::Check,
                Color32::WHITE,
            );
        }
    }
}

fn line_icon(ui: &mut egui::Ui, icon: LineIcon, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_line_icon(ui.painter(), rect, icon, color);
}

fn icon_only_button(ui: &mut egui::Ui, icon: LineIcon, hover_text: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 6.0, Color32::from_rgb(236, 240, 237));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    paint_line_icon(ui.painter(), rect.shrink(6.0), icon, MUTED);
    response.on_hover_text(hover_text)
}

fn icon_text_button(ui: &mut egui::Ui, icon: LineIcon, label: &str) -> egui::Response {
    secondary_button(ui, Some(icon), label)
}

fn secondary_button(ui: &mut egui::Ui, icon: Option<LineIcon>, label: &str) -> egui::Response {
    let font = FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), TEXT);
    let icon_space = if icon.is_some() { 22.0 } else { 0.0 };
    let width = galley.size().x + icon_space + 24.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width.max(72.0), 34.0), Sense::click());
    let fill = if response.hovered() {
        Color32::from_rgb(241, 244, 241)
    } else {
        SURFACE
    };
    ui.painter().rect(
        rect,
        7.0,
        fill,
        Stroke::new(1.0, BORDER),
        StrokeKind::Inside,
    );
    let mut text_x = rect.center().x - galley.size().x / 2.0;
    if let Some(icon) = icon {
        let icon_rect = Rect::from_center_size(
            Pos2::new(
                rect.center().x - (galley.size().x + icon_space) / 2.0 + 8.0,
                rect.center().y,
            ),
            Vec2::splat(16.0),
        );
        paint_line_icon(ui.painter(), icon_rect, icon, MUTED);
        text_x += icon_space / 2.0;
    }
    ui.painter().galley(
        Pos2::new(
            text_x,
            rect.center().y - galley.size().y / 2.0 + OPTICAL_TEXT_OFFSET_Y,
        ),
        galley,
        TEXT,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn status_message(ui: &mut egui::Ui, icon: LineIcon, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        line_icon(ui, icon, 15.0, color);
        optical_label(ui, RichText::new(text).size(13.0).color(color));
    });
}

fn paint_line_icon(painter: &egui::Painter, rect: Rect, icon: LineIcon, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let x = |value: f32| rect.left() + rect.width() * value;
    let y = |value: f32| rect.top() + rect.height() * value;
    let point = |x_value: f32, y_value: f32| Pos2::new(x(x_value), y(y_value));
    match icon {
        LineIcon::Brand => {
            painter.rect_stroke(rect.shrink(2.0), 4.0, stroke, StrokeKind::Inside);
            painter.line_segment([point(0.32, 0.34), point(0.68, 0.34)], stroke);
            painter.line_segment([point(0.32, 0.52), point(0.58, 0.52)], stroke);
            painter.circle_filled(point(0.68, 0.68), 1.6, color);
        }
        LineIcon::Import => {
            painter.rect_stroke(
                Rect::from_min_max(point(0.13, 0.2), point(0.55, 0.8)),
                2.0,
                stroke,
                StrokeKind::Inside,
            );
            painter.line_segment([point(0.42, 0.5), point(0.88, 0.5)], stroke);
            painter.line_segment([point(0.7, 0.32), point(0.88, 0.5)], stroke);
            painter.line_segment([point(0.7, 0.68), point(0.88, 0.5)], stroke);
        }
        LineIcon::Export => {
            painter.line_segment([point(0.5, 0.12), point(0.5, 0.65)], stroke);
            painter.line_segment([point(0.3, 0.45), point(0.5, 0.65)], stroke);
            painter.line_segment([point(0.7, 0.45), point(0.5, 0.65)], stroke);
            painter.line_segment([point(0.18, 0.82), point(0.82, 0.82)], stroke);
            painter.line_segment([point(0.18, 0.82), point(0.18, 0.66)], stroke);
            painter.line_segment([point(0.82, 0.82), point(0.82, 0.66)], stroke);
        }
        LineIcon::Recovery => {
            painter.circle_stroke(
                rect.center(),
                rect.width().min(rect.height()) * 0.36,
                stroke,
            );
            paint_line_icon(
                painter,
                Rect::from_min_max(point(0.28, 0.29), point(0.72, 0.7)),
                LineIcon::Check,
                color,
            );
        }
        LineIcon::Logs => {
            for offset in [0.28, 0.5, 0.72] {
                painter.circle_filled(point(0.18, offset), 1.2, color);
                painter.line_segment([point(0.3, offset), point(0.84, offset)], stroke);
            }
        }
        LineIcon::Folder => {
            painter.line_segment([point(0.1, 0.34), point(0.1, 0.8)], stroke);
            painter.line_segment([point(0.1, 0.8), point(0.9, 0.8)], stroke);
            painter.line_segment([point(0.9, 0.8), point(0.9, 0.38)], stroke);
            painter.line_segment([point(0.9, 0.38), point(0.5, 0.38)], stroke);
            painter.line_segment([point(0.5, 0.38), point(0.4, 0.23)], stroke);
            painter.line_segment([point(0.4, 0.23), point(0.1, 0.23)], stroke);
            painter.line_segment([point(0.1, 0.23), point(0.1, 0.34)], stroke);
        }
        LineIcon::ChevronDown => {
            painter.line_segment([point(0.25, 0.4), point(0.5, 0.65)], stroke);
            painter.line_segment([point(0.5, 0.65), point(0.75, 0.4)], stroke);
        }
        LineIcon::ChevronRight => {
            painter.line_segment([point(0.4, 0.25), point(0.65, 0.5)], stroke);
            painter.line_segment([point(0.65, 0.5), point(0.4, 0.75)], stroke);
        }
        LineIcon::Check => {
            painter.line_segment([point(0.15, 0.52), point(0.4, 0.75)], stroke);
            painter.line_segment([point(0.4, 0.75), point(0.86, 0.25)], stroke);
        }
        LineIcon::Warning => {
            painter.line_segment([point(0.5, 0.12), point(0.08, 0.85)], stroke);
            painter.line_segment([point(0.08, 0.85), point(0.92, 0.85)], stroke);
            painter.line_segment([point(0.92, 0.85), point(0.5, 0.12)], stroke);
            painter.line_segment([point(0.5, 0.36), point(0.5, 0.6)], stroke);
            painter.circle_filled(point(0.5, 0.72), 1.2, color);
        }
        LineIcon::Repair => {
            painter.line_segment([point(0.18, 0.78), point(0.62, 0.34)], stroke);
            painter.circle_stroke(point(0.7, 0.26), rect.width() * 0.18, stroke);
            painter.line_segment([point(0.16, 0.8), point(0.34, 0.82)], stroke);
        }
        LineIcon::Html => {
            painter.rect_stroke(rect.shrink(2.0), 2.0, stroke, StrokeKind::Inside);
            painter.line_segment([point(0.38, 0.38), point(0.22, 0.5)], stroke);
            painter.line_segment([point(0.22, 0.5), point(0.38, 0.62)], stroke);
            painter.line_segment([point(0.62, 0.38), point(0.78, 0.5)], stroke);
            painter.line_segment([point(0.78, 0.5), point(0.62, 0.62)], stroke);
        }
        LineIcon::Settings => {
            painter.circle_stroke(rect.center(), rect.width() * 0.2, stroke);
            for angle in 0..8 {
                let radians = angle as f32 * std::f32::consts::TAU / 8.0;
                let direction = Vec2::angled(radians);
                painter.line_segment(
                    [
                        rect.center() + direction * rect.width() * 0.31,
                        rect.center() + direction * rect.width() * 0.43,
                    ],
                    stroke,
                );
            }
        }
    }
}

fn sticky_action_bar(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(content);
        });
}

fn primary_action(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    filled_action(ui, text, enabled, ACCENT, Color32::from_rgb(25, 96, 90))
}

fn danger_action(ui: &mut egui::Ui, text: &str) -> egui::Response {
    filled_action(ui, text, true, DANGER, Color32::from_rgb(149, 50, 45))
}

fn filled_action(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    normal_fill: Color32,
    hover_fill: Color32,
) -> egui::Response {
    let font = FontId::proportional(14.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let width = (galley.size().x + 32.0).max(136.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 36.0), sense);
    let fill = if !enabled {
        Color32::from_rgb(132, 183, 178)
    } else if response.hovered() {
        hover_fill
    } else {
        normal_fill
    };
    ui.painter().rect_filled(rect, 7.0, fill);
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0 + OPTICAL_TEXT_OFFSET_Y,
        ),
        galley,
        Color32::WHITE,
    );
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn summary_metric(ui: &mut egui::Ui, label: &str, value: usize) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(9.0)
        .inner_margin(Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_min_width(120.0);
            ui.label(
                RichText::new(value.to_string())
                    .size(22.0)
                    .strong()
                    .color(TEXT),
            );
            ui.label(RichText::new(label).small().color(MUTED));
        });
}

fn confirmation(
    context: &egui::Context,
    spec: ConfirmationSpec<'_>,
    open: &mut bool,
    mut confirm: impl FnMut(),
) {
    let ConfirmationSpec {
        title,
        message,
        details,
        action,
        destructive,
        chinese,
    } = spec;
    let response = egui::Modal::new(egui::Id::new(("confirmation", title)))
        .backdrop_color(Color32::from_black_alpha(72))
        .frame(
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(14.0)
                .inner_margin(Margin::symmetric(24, 22)),
        )
        .show(context, |ui| {
            ui.set_width(430.0);
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(if destructive {
                        Color32::from_rgb(249, 235, 233)
                    } else {
                        Color32::from_rgb(228, 241, 238)
                    })
                    .corner_radius(9.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        line_icon(
                            ui,
                            if destructive {
                                LineIcon::Warning
                            } else {
                                LineIcon::Import
                            },
                            20.0,
                            if destructive { DANGER } else { ACCENT },
                        );
                    });
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    optical_label(ui, RichText::new(title).size(18.0).strong().color(TEXT));
                    ui.label(
                        RichText::new(if destructive {
                            tr(chinese, "请确认此操作", "Confirm this action")
                        } else {
                            tr(chinese, "最后一步", "Final step")
                        })
                        .size(12.0)
                        .color(MUTED),
                    );
                });
            });
            ui.add_space(16.0);
            ui.label(RichText::new(message).size(13.0).color(TEXT));
            ui.add_space(14.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(247, 248, 245))
                .corner_radius(9.0)
                .inner_margin(Margin::symmetric(14, 11))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for detail in details {
                        ui.horizontal(|ui| {
                            line_icon(
                                ui,
                                if destructive {
                                    LineIcon::Warning
                                } else {
                                    LineIcon::Check
                                },
                                14.0,
                                if destructive { DANGER } else { ACCENT },
                            );
                            optical_label(ui, RichText::new(*detail).size(13.0).color(TEXT));
                        });
                    }
                });
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let confirmed = if destructive {
                    danger_action(ui, action).clicked()
                } else {
                    primary_action(ui, action, true).clicked()
                };
                if confirmed {
                    *open = false;
                    confirm();
                }
                if secondary_button(ui, None, tr(chinese, "取消", "Cancel")).clicked() {
                    *open = false;
                }
            });
        });
    if response.should_close() {
        *open = false;
    }
}

fn import_success_modal(
    context: &egui::Context,
    summary: &ImportSummary,
    close: &mut bool,
    zh: bool,
) {
    let response = egui::Modal::new(egui::Id::new("import-success"))
        .backdrop_color(Color32::from_black_alpha(72))
        .frame(
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(14.0)
                .inner_margin(Margin::symmetric(24, 22)),
        )
        .show(context, |ui| {
            ui.set_width(430.0);
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(228, 241, 238))
                    .corner_radius(24.0)
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        line_icon(ui, LineIcon::Check, 22.0, ACCENT);
                    });
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    optical_label(
                        ui,
                        RichText::new(tr(zh, "操作完成", "Operation completed"))
                            .size(18.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new(tr(
                            zh,
                            "会话元数据已安全更新",
                            "Session metadata was updated safely",
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                });
            });
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                summary_metric(ui, tr(zh, "新增会话", "New sessions"), summary.imported);
                ui.add_space(8.0);
                summary_metric(ui, tr(zh, "刷新索引", "Refreshed"), summary.refreshed);
                ui.add_space(8.0);
                summary_metric(ui, tr(zh, "重复跳过", "Duplicates"), summary.skipped);
            });
            ui.add_space(14.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(247, 248, 245))
                .corner_radius(9.0)
                .inner_margin(Margin::symmetric(14, 11))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(tr(zh, "回滚事务", "Rollback transaction"))
                            .size(12.0)
                            .color(MUTED),
                    );
                    ui.label(
                        RichText::new(short_id(&summary.transaction_id))
                            .size(13.0)
                            .monospace()
                            .color(TEXT),
                    );
                    ui.add_space(5.0);
                    ui.label(
                        RichText::new(tr(
                            zh,
                            "重新打开 Codex 后即可查看更新后的项目与会话。",
                            "Reopen Codex to view the updated projects and sessions.",
                        ))
                        .size(13.0)
                        .color(TEXT),
                    );
                });
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_action(ui, tr(zh, "完成", "Done"), true).clicked() {
                    *close = true;
                }
            });
        });
    if response.should_close() {
        *close = true;
    }
}

fn completion_notice_modal(
    context: &egui::Context,
    notice: &CompletionNotice,
    close: &mut bool,
    zh: bool,
) {
    let response = egui::Modal::new(egui::Id::new("task-completion"))
        .backdrop_color(Color32::from_black_alpha(72))
        .frame(
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(14.0)
                .inner_margin(Margin::symmetric(24, 22)),
        )
        .show(context, |ui| {
            ui.set_width(430.0);
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(228, 241, 238))
                    .corner_radius(24.0)
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        line_icon(ui, LineIcon::Check, 22.0, ACCENT);
                    });
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    optical_label(
                        ui,
                        RichText::new(&notice.title).size(18.0).strong().color(TEXT),
                    );
                    ui.label(RichText::new(&notice.message).size(13.0).color(MUTED));
                });
            });
            if !notice.detail.is_empty() {
                ui.add_space(16.0);
                egui::Frame::new()
                    .fill(Color32::from_rgb(247, 248, 245))
                    .corner_radius(9.0)
                    .inner_margin(Margin::symmetric(14, 11))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(tr(zh, "输出位置", "Output location"))
                                .size(12.0)
                                .color(MUTED),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(&notice.detail).size(13.0).color(TEXT))
                                .wrap(),
                        );
                    });
            }
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_action(ui, tr(zh, "完成", "Done"), true).clicked() {
                    *close = true;
                }
            });
        });
    if response.should_close() {
        *close = true;
    }
}

fn action_label(action: &MergeAction, zh: bool) -> &'static str {
    match action {
        MergeAction::Import => tr(zh, "新增", "New"),
        MergeAction::SkipIdentical => tr(zh, "重复，刷新索引", "Duplicate, refresh index"),
        MergeAction::ReplaceWithLonger => tr(zh, "更新为较长版本", "Use longer version"),
        MergeAction::KeepTargetLonger => tr(zh, "保留并刷新索引", "Keep and refresh"),
        MergeAction::Conflict => tr(zh, "冲突", "Conflict"),
    }
}

fn action_badge(ui: &mut egui::Ui, action: &MergeAction, zh: bool) {
    let color = action_color(action);
    let fill = match action {
        MergeAction::Import | MergeAction::ReplaceWithLonger => Color32::from_rgb(228, 241, 238),
        MergeAction::Conflict => Color32::from_rgb(249, 235, 233),
        MergeAction::SkipIdentical | MergeAction::KeepTargetLonger => {
            Color32::from_rgb(238, 240, 237)
        }
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(12.0)
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(
                RichText::new(action_label(action, zh))
                    .size(12.0)
                    .strong()
                    .color(color),
            );
        });
}

fn action_color(action: &MergeAction) -> Color32 {
    match action {
        MergeAction::Import | MergeAction::ReplaceWithLonger => ACCENT,
        MergeAction::SkipIdentical | MergeAction::KeepTargetLonger => MUTED,
        MergeAction::Conflict => Color32::from_rgb(180, 64, 58),
    }
}

fn format_date(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "Unknown / 日期未知".to_owned())
}

fn optional_path(value: &str) -> Option<PathBuf> {
    (!value.trim().is_empty()).then(|| PathBuf::from(value.trim()))
}

fn short_id(value: &str) -> &str {
    value.get(..value.len().min(24)).unwrap_or(value)
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn configure_style(context: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = Color32::from_rgb(241, 242, 239);
    visuals.selection.bg_fill = ACCENT;
    visuals.hyperlink_color = ACCENT;
    visuals.widgets.inactive.corner_radius = 7.0.into();
    visuals.widgets.hovered.corner_radius = 7.0.into();
    visuals.widgets.active.corner_radius = 7.0.into();
    context.set_visuals(visuals);

    let mut style = (*context.style()).clone();
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(24.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(12.0, FontFamily::Proportional),
    );
    style.spacing.item_spacing = Vec2::new(8.0, 7.0);
    style.spacing.button_padding = Vec2::new(12.0, 7.0);
    style.spacing.interact_size.y = 34.0;
    context.set_style(style);
}

fn install_system_font(context: &egui::Context) {
    let candidates = if cfg!(target_os = "macos") {
        vec![
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
        ]
    } else if cfg!(target_os = "windows") {
        vec![r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simhei.ttf"]
    } else {
        vec![
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ]
    };
    let Some(bytes) = candidates
        .into_iter()
        .find_map(|path| std::fs::read(path).ok())
    else {
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("system-ui".to_owned(), FontData::from_owned(bytes).into());
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "system-ui".to_owned());
    context.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_migrate::model::{SourceSession, ThreadRecord};

    #[test]
    fn project_selection_supports_all_partial_and_none() {
        let mut project = UiProject {
            original_cwd: "/old/project".to_owned(),
            target_path: String::new(),
            history_only: false,
            expanded: true,
            sessions: vec![session("one"), session("two")],
        };
        assert_eq!(project.state(), CheckState::All);

        project.sessions[0].selected = false;
        assert_eq!(project.state(), CheckState::Partial);

        project.set_selected(false);
        assert_eq!(project.state(), CheckState::None);

        project.set_selected(true);
        assert_eq!(project.state(), CheckState::All);
    }

    #[test]
    fn common_parent_and_parent_mapping_match_existing_projects() {
        assert_eq!(
            common_parent(["/old/projects/a", "/old/projects/b"].into_iter()).as_deref(),
            Some("/old/projects")
        );
        let root = tempfile::tempdir().unwrap();
        let project_path = root.path().join("demo");
        std::fs::create_dir_all(&project_path).unwrap();
        let mut projects = vec![UiProject {
            original_cwd: "/old/projects/demo".to_owned(),
            target_path: String::new(),
            history_only: false,
            expanded: true,
            sessions: vec![session("one")],
        }];
        apply_parent_mapping_to(
            &mut projects,
            "/old/projects",
            root.path().to_string_lossy().as_ref(),
        );
        assert_eq!(projects[0].target_path, project_path.to_string_lossy());
    }

    #[test]
    fn path_problem_filter_detects_missing_project_folder() {
        let project = UiProject {
            original_cwd: "/definitely/missing/codex-migrate-project".to_owned(),
            target_path: String::new(),
            history_only: false,
            expanded: true,
            sessions: vec![session("one")],
        };
        assert!(project_has_path_problem(&project));
    }

    #[test]
    fn html_selection_defaults_to_none_and_supports_select_all() {
        let mut projects = vec![UiProject {
            original_cwd: "/project".to_owned(),
            target_path: "/project".to_owned(),
            history_only: false,
            expanded: true,
            sessions: vec![session("one"), session("two")],
        }];
        set_projects_selected(&mut projects, false);
        assert_eq!(projects_selection_state(&projects), CheckState::None);
        set_projects_selected(&mut projects, true);
        assert_eq!(projects_selection_state(&projects), CheckState::All);
        projects[0].sessions[0].selected = false;
        assert_eq!(projects_selection_state(&projects), CheckState::Partial);
    }

    fn session(id: &str) -> UiSession {
        UiSession {
            selected: true,
            source: SourceSession {
                source_path: format!("/source/{id}.jsonl"),
                thread: ThreadRecord {
                    id: id.to_owned(),
                    title: id.to_owned(),
                    created_at: 0,
                    updated_at: 0,
                    cwd: "/old/project".to_owned(),
                    source: "test".to_owned(),
                    thread_source: Some("user".to_owned()),
                    model_provider: "openai".to_owned(),
                    cli_version: String::new(),
                    archived: false,
                    archive_path: format!("sessions/{id}.jsonl"),
                    sha256: String::new(),
                    byte_len: 0,
                    first_user_message: String::new(),
                    sandbox_policy: None,
                    approval_mode: None,
                    model: None,
                    reasoning_effort: None,
                },
            },
        }
    }
}
