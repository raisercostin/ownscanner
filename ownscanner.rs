//! OwnScanner: start, feed documents, see each image, fix paper problems, continue.
//!
//! Build: rustc --edition 2024 ownscanner.rs -O -o ownscanner.exe
//! Test:  ./ownscanner --dev-autoformat --dev-autoupdate --dev-test

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = "0.1.0";
const WAIT_POLL_MS: u32 = 250;
const IDAT_BYTES: usize = 64 * 1024;

mod dev_autoupdate {
    use std::env;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{self, Command, ExitStatus};
    use std::thread;
    use std::time::Duration;

    fn executable_name(binary_stem: &str) -> String {
        format!("{binary_stem}{}", if cfg!(windows) { ".exe" } else { "" })
    }

    fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
        let before = args.len();
        args.retain(|arg| arg != flag);
        args.len() != before
    }

    fn take_option(args: &mut Vec<String>, flag: &str) -> io::Result<Option<String>> {
        let Some(index) = args.iter().position(|arg| arg == flag) else {
            return Ok(None);
        };
        if index + 1 >= args.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{flag} requires a value"),
            ));
        }
        args.remove(index);
        Ok(Some(args.remove(index)))
    }

    fn remove_with_retry(path: &Path) -> io::Result<()> {
        for attempt in 0..50 {
            match fs::remove_file(path) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) if attempt == 49 => return Err(error),
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        }
        unreachable!()
    }

    fn copy_with_retry(source: &Path, target: &Path) -> io::Result<()> {
        for attempt in 0..50 {
            match fs::copy(source, target) {
                Ok(_) => return Ok(()),
                Err(error) if attempt == 49 => return Err(error),
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        }
        unreachable!()
    }

    fn source_and_target(
        exe: &Path,
        project: &Path,
        source_file: &str,
        binary_stem: &str,
    ) -> Option<(PathBuf, PathBuf)> {
        let project_source = project.join(source_file);
        let project_target = project.join(executable_name(binary_stem));

        // An installed executable follows the selected project's source.
        if exe != project_target && project_source.is_file() {
            return Some((project_source, project_target));
        }

        // A local executable follows source adjacent to itself.
        let adjacent_source = exe.with_file_name(source_file);
        adjacent_source
            .is_file()
            .then_some((adjacent_source, exe.to_path_buf()))
    }

    fn maybe_dev_autoupdate(
        args: &mut Vec<String>,
        source_file: &str,
        binary_stem: &str,
    ) -> io::Result<Option<ExitStatus>> {
        let autoupdate = take_flag(args, "--dev-autoupdate");
        let last = take_flag(args, "--last");
        if !autoupdate && !last {
            return Ok(None);
        }

        let exe = env::current_exe()?;
        let project = env::current_dir()?;
        let Some((source, target)) = source_and_target(&exe, &project, source_file, binary_stem)
        else {
            return Ok(None);
        };
        if source == target {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to compile over the source file",
            ));
        }

        let stale = !target.exists()
            || fs::metadata(&source)?.modified()? > fs::metadata(&target)?.modified()?;
        if !stale && target == exe {
            return Ok(None);
        }

        let staged = target.with_file_name(format!(
            "{binary_stem}.next.{}{}",
            process::id(),
            if cfg!(windows) { ".exe" } else { "" }
        ));
        if stale {
            let compiled = Command::new("rustc")
                .args(["--edition", "2024", "-O"])
                .arg(&source)
                .arg("-o")
                .arg(&staged)
                .status()?;
            if !compiled.success() {
                remove_with_retry(&staged)?;
                return Err(io::Error::other(format!(
                    "rustc failed with status {compiled}"
                )));
            }
        }

        if stale && target != exe {
            copy_with_retry(&staged, &target)?;
            remove_with_retry(&staged)?;
        }
        let command = if stale && target == exe {
            &staged
        } else {
            &target
        };
        let status = Command::new(command).args(&*args).status()?;

        // The staged process performs replacement only after the requested command exits.
        if stale && target == exe {
            Command::new(&staged)
                .arg("--savepoint")
                .arg(&target)
                .spawn()?;
        }
        Ok(Some(status))
    }

    fn savepoint(target: PathBuf) -> io::Result<()> {
        let staged = env::current_exe()?;
        copy_with_retry(&staged, &target)?;
        Command::new(&target)
            .arg("--cleanup")
            .arg(&staged)
            .arg("--replacement-complete")
            .spawn()?;
        Ok(())
    }

    pub fn handle(
        args: &mut Vec<String>,
        source_file: &str,
        binary_stem: &str,
    ) -> io::Result<Option<i32>> {
        if let Some(target) = take_option(args, "--savepoint")? {
            savepoint(PathBuf::from(target))?;
            return Ok(Some(0));
        }
        if let Some(staged) = take_option(args, "--cleanup")? {
            remove_with_retry(Path::new(&staged))?;
        }
        if take_flag(args, "--replacement-complete") {
            return Ok(Some(0));
        }
        if let Some(status) = maybe_dev_autoupdate(args, source_file, binary_stem)? {
            return Ok(Some(status.code().unwrap_or(1)));
        }
        Ok(None)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn root(name: &str) -> PathBuf {
            env::temp_dir().join(format!(
                "dev-autoupdate-{name}-{}-{}",
                process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
        }

        #[test]
        fn consumes_all_update_aliases() {
            let mut args = ["--last", "list", "--dev-autoupdate", "--last"]
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
            assert!(take_flag(&mut args, "--dev-autoupdate"));
            assert!(take_flag(&mut args, "--last"));
            assert_eq!(args, ["list"]);
        }

        #[test]
        fn installed_binary_prefers_project_source() -> io::Result<()> {
            let root = root("project-source");
            let project = root.join("project");
            let installed = root.join("installed").join(executable_name("tool"));
            fs::create_dir_all(installed.parent().unwrap())?;
            fs::create_dir_all(&project)?;
            fs::write(installed.with_file_name("tool.rs"), "packaged")?;
            fs::write(project.join("tool.rs"), "project")?;
            assert_eq!(
                source_and_target(&installed, &project, "tool.rs", "tool"),
                Some((
                    project.join("tool.rs"),
                    project.join(executable_name("tool"))
                ))
            );
            fs::remove_dir_all(root)
        }

        #[test]
        fn local_binary_uses_adjacent_source() -> io::Result<()> {
            let root = root("adjacent-source");
            fs::create_dir_all(&root)?;
            let executable = root.join(executable_name("tool"));
            fs::write(root.join("tool.rs"), "source")?;
            assert_eq!(
                source_and_target(&executable, &root, "tool.rs", "tool"),
                Some((root.join("tool.rs"), executable))
            );
            fs::remove_dir_all(root)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    Rgb24,
    Gray8,
}

impl ColorMode {
    fn channels(self) -> usize {
        match self {
            Self::Rgb24 => 3,
            Self::Gray8 => 1,
        }
    }

    fn png_type(self) -> u8 {
        match self {
            Self::Rgb24 => 2,
            Self::Gray8 => 0,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Rgb24 => "rgb24",
            Self::Gray8 => "gray8",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Front,
    Back,
}

impl Side {
    fn tag(self) -> &'static str {
        match self {
            Self::Front => "Fron",
            Self::Back => "Back",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    Jam,
    DoubleFeed,
    Skew,
    PaperFeedError,
    LengthLimitReached,
    DeviceFailure,
    NoMedia,
}

impl StopReason {
    fn name(self) -> &'static str {
        match self {
            Self::NoMedia => "no-media",
            Self::Jam => "jam",
            Self::DoubleFeed => "double-feed",
            Self::Skew => "skew",
            Self::PaperFeedError => "paper-feed-error",
            Self::LengthLimitReached => "length-limit-reached",
            Self::DeviceFailure => "device-failure",
        }
    }

    fn instruction(self) -> &'static str {
        match self {
            Self::NoMedia => "The scanner did not pick up the paper. Check that it is seated.",
            Self::Jam => {
                "Paper is stuck. Remove the feeder stack, open the scanner, remove the stuck paper, close it, then reload."
            }
            Self::DoubleFeed => "Separate the sheets and reload them.",
            Self::Skew => "Straighten the sheet and reload it.",
            Self::PaperFeedError => "Check the paper and reload it.",
            Self::LengthLimitReached => "Remove the document, then load the next one.",
            Self::DeviceFailure => "Check the scanner connection and power.",
        }
    }
}

/// The raw capture settings. Optional image processing stays off by default.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub scanner: Option<String>,
    pub read_ahead: bool,
    pub directory: PathBuf,
    pub prefix: String,
    pub color: ColorMode,
    pub dpi: u32,
    pub duplex: bool,
    pub max_length_mm: u32,
    pub margin_mm: f64,
    pub skip_blank: bool,
    pub deskew: bool,
    pub edge_emphasis: u8,
    pub remove_shadow: bool,
    pub color_unevenness_reduction: bool,
    pub bleed_through_removal: bool,
}

impl Profile {
    pub fn long_raw(directory: impl Into<PathBuf>) -> Self {
        Self {
            name: "long-raw".into(),
            scanner: None,
            read_ahead: true,
            directory: directory.into(),
            prefix: "scan".into(),
            color: ColorMode::Rgb24,
            dpi: 600,
            duplex: true,
            max_length_mm: 5588,
            margin_mm: 0.0,
            skip_blank: false,
            deskew: false,
            edge_emphasis: 3,
            remove_shadow: false,
            color_unevenness_reduction: false,
            bleed_through_removal: false,
        }
    }

    fn validate(&self) -> io::Result<()> {
        let invalid = |message: String| Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        if self.dpi == 0 {
            return invalid("dpi must be positive".into());
        }
        if self.max_length_mm == 0 {
            return invalid("maximum length must be positive".into());
        }
        if !(1..=5).contains(&self.edge_emphasis) {
            return invalid("edge emphasis must be from 1 to 5".into());
        }
        if !self.margin_mm.is_finite() {
            return invalid("margin must be finite".into());
        }
        if self.prefix.is_empty()
            || self
                .prefix
                .contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        {
            return invalid(format!("invalid prefix `{}`", self.prefix));
        }
        Ok(())
    }

    fn active_processing(&self) -> Vec<&'static str> {
        [
            ("skip_blank", self.skip_blank),
            ("deskew", self.deskew),
            ("edge_emphasis", self.edge_emphasis != 3),
            ("remove_shadow", self.remove_shadow),
            (
                "color_unevenness_reduction",
                self.color_unevenness_reduction,
            ),
            ("bleed_through_removal", self.bleed_through_removal),
        ]
        .into_iter()
        .filter_map(|(name, active)| active.then_some(name))
        .collect()
    }

    fn max_rows(&self) -> u32 {
        (f64::from(self.max_length_mm) / 25.4 * f64::from(self.dpi)).round() as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SideInfo {
    pub side: Side,
    pub width: u32,
    pub dpi: u32,
    pub color: ColorMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceStep {
    Side(SideInfo),
    FeederEmpty,
    Stopped(StopReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStep {
    Row,
    End,
    Stopped(StopReason),
}

/// The scanner boundary used by the real TWAIN adapter and test scanner.
pub trait ScannerDevice {
    fn identity(&self) -> String;
    fn configure(&mut self, profile: &Profile) -> io::Result<()>;
    fn next_side(&mut self) -> io::Result<DeviceStep>;
    fn read_row(&mut self, row: &mut [u8]) -> io::Result<RowStep>;
    fn abort_side(&mut self) -> io::Result<()>;
    fn wait_for_paper(&mut self, timeout_ms: u32) -> io::Result<bool>;

    /// Extension point for model-specific recovery help.
    fn instruction(&self, _reason: StopReason) -> Option<String> {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SideStatus {
    Complete,
    Partial,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedSide {
    pub sheet: u32,
    pub side: Side,
    pub status: SideStatus,
    pub file: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub sha256: String,
    pub stop: Option<StopReason>,
    pub timing: SideTiming,
}

/// Where one side spent its time, so a slow sheet can be explained from the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SideTiming {
    pub driver_wait: Duration,
    pub transfer: Duration,
    pub compress: Duration,
    pub hash: Duration,
    pub publish: Duration,
}

impl SideTiming {
    fn label(&self) -> Option<String> {
        (*self != Self::default()).then(|| {
            format!(
                "wait {:.2}s, transfer {:.2}s, compress+write {:.2}s, hash {:.2}s, publish {:.3}s",
                self.driver_wait.as_secs_f64(),
                self.transfer.as_secs_f64(),
                self.compress.as_secs_f64(),
                self.hash.as_secs_f64(),
                self.publish.as_secs_f64()
            )
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEvent {
    Started { batch: String },
    Scanning { sheet: u32, side: Side },
    Saved(SavedSide),
    NeedsAttention { reason: StopReason, message: String },
    WaitingForPaper,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchReport {
    pub batch: String,
    pub manifest: PathBuf,
    pub sides: Vec<SavedSide>,
}

/// Application facade. One batch stays open while paper is added or problems are fixed.
pub struct OwnScannerApp<D> {
    device: D,
    stamp: Box<dyn FnMut() -> String>,
}

impl<D: ScannerDevice> OwnScannerApp<D> {
    pub fn with_stamp(device: D, stamp: impl FnMut() -> String + 'static) -> Self {
        Self {
            device,
            stamp: Box::new(stamp),
        }
    }

    /// Runs until the observer returns [`Flow::Stop`].
    pub fn run_batch(
        &mut self,
        profile: &Profile,
        observer: &mut dyn FnMut(&AppEvent) -> Flow,
    ) -> io::Result<BatchReport> {
        profile.validate()?;
        fs::create_dir_all(&profile.directory)?;
        self.device.configure(profile)?;
        let batch = (self.stamp)();
        let manifest = profile
            .directory
            .join(format!("{}_{batch}.manifest.json", profile.prefix));
        let identity = self.device.identity();
        let mut report = BatchReport {
            batch: batch.clone(),
            manifest,
            sides: Vec::new(),
        };
        write_manifest(&report, profile, &identity, "open", true)?;

        let mut next_sheet = 1;
        let mut open_sheet = None;
        let mut flow = observer(&AppEvent::Started { batch });
        while flow == Flow::Continue {
            let stopped = match self.device.next_side() {
                Ok(DeviceStep::Side(info)) => {
                    let sheet = match info.side {
                        Side::Front => {
                            let sheet = next_sheet;
                            next_sheet += 1;
                            open_sheet = Some(sheet);
                            sheet
                        }
                        Side::Back => open_sheet.take().ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "back side without front side",
                            )
                        })?,
                    };
                    flow = observer(&AppEvent::Scanning {
                        sheet,
                        side: info.side,
                    });
                    let saved = self.capture_side(profile, &report.batch, sheet, info)?;
                    let stop = saved.as_ref().and_then(|side| side.stop);
                    if let Some(saved) = saved {
                        if saved.height == 0 && saved.side == Side::Front {
                            // No paper was captured, so this attempt is not a sheet.
                            next_sheet = sheet;
                            open_sheet = None;
                        } else {
                            report.sides.push(saved.clone());
                            write_manifest(&report, profile, &identity, "open", false)?;
                            if observer(&AppEvent::Saved(saved)) == Flow::Stop {
                                flow = Flow::Stop;
                            }
                        }
                    }
                    stop
                }
                Ok(DeviceStep::FeederEmpty) => {
                    open_sheet = None;
                    if flow == Flow::Continue {
                        flow = self.wait_for_paper(observer);
                    }
                    None
                }
                Ok(DeviceStep::Stopped(reason)) => Some(reason),
                Err(_) => Some(StopReason::DeviceFailure),
            };
            if let Some(reason) = stopped {
                open_sheet = None;
                let message = self
                    .device
                    .instruction(reason)
                    .unwrap_or_else(|| reason.instruction().to_owned());
                if observer(&AppEvent::NeedsAttention { reason, message }) == Flow::Stop {
                    flow = Flow::Stop;
                }
                if flow == Flow::Continue {
                    flow = self.wait_for_paper(observer);
                }
            }
        }
        write_manifest(&report, profile, &identity, "finished", false)?;
        observer(&AppEvent::Finished);
        Ok(report)
    }

    fn wait_for_paper(&mut self, observer: &mut dyn FnMut(&AppEvent) -> Flow) -> Flow {
        loop {
            if matches!(self.device.wait_for_paper(WAIT_POLL_MS), Ok(true)) {
                return Flow::Continue;
            }
            if observer(&AppEvent::WaitingForPaper) == Flow::Stop {
                return Flow::Stop;
            }
        }
    }

    fn capture_side(
        &mut self,
        profile: &Profile,
        batch: &str,
        sheet: u32,
        info: SideInfo,
    ) -> io::Result<Option<SavedSide>> {
        if info.width == 0 || info.dpi != profile.dpi || info.color != profile.color {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("scanner side {info:?} does not match the profile"),
            ));
        }
        let base = format!(
            "{}_{batch}_{sheet:04}_{}_{}_None",
            profile.prefix,
            info.side.tag(),
            info.dpi
        );
        let temp = profile.directory.join(format!(".{base}.png.tmp"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let captured = capture_rows(&mut self.device, file, info, profile.max_rows());
        let (height, stop, mut timing) = match captured {
            Ok(result) => result,
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error);
            }
        };
        if height == 0 {
            fs::remove_file(&temp)?;
            return Ok(stop.map(|reason| SavedSide {
                sheet,
                side: info.side,
                status: SideStatus::Partial,
                file: String::new(),
                width: info.width,
                height: 0,
                bytes: 0,
                sha256: String::new(),
                stop: Some(reason),
                timing,
            }));
        }
        let (status, name) = match stop {
            None => (SideStatus::Complete, format!("{base}.png")),
            Some(_) => (SideStatus::Partial, format!("{base}_partial.png")),
        };
        let started = std::time::Instant::now();
        let (sha256, bytes) = sha256_file(&temp)?;
        timing.hash = started.elapsed();
        let started = std::time::Instant::now();
        publish_no_clobber(&temp, &profile.directory.join(&name))?;
        timing.publish = started.elapsed();
        Ok(Some(SavedSide {
            sheet,
            side: info.side,
            status,
            file: name,
            width: info.width,
            height,
            bytes,
            sha256,
            stop,
            timing,
        }))
    }
}

fn capture_rows<D: ScannerDevice>(
    device: &mut D,
    file: File,
    info: SideInfo,
    max_rows: u32,
) -> io::Result<(u32, Option<StopReason>, SideTiming)> {
    let mut row = vec![0; info.width as usize * info.color.channels()];
    let mut png = PngStream::new(file, info.width, info.color, info.dpi)?;
    let mut timing = SideTiming::default();
    let mut first_row_seen = false;
    let stop = loop {
        let started = std::time::Instant::now();
        let step = device.read_row(&mut row);
        if first_row_seen {
            timing.transfer += started.elapsed();
        } else {
            timing.driver_wait += started.elapsed();
            first_row_seen = matches!(step, Ok(RowStep::Row));
        }
        match step {
            Ok(RowStep::Row) if png.rows == max_rows => {
                let _ = device.abort_side();
                break Some(StopReason::LengthLimitReached);
            }
            Ok(RowStep::Row) => {
                let started = std::time::Instant::now();
                png.write_row(&row)?;
                timing.compress += started.elapsed();
            }
            Ok(RowStep::End) => break None,
            Ok(RowStep::Stopped(reason)) => break Some(reason),
            Err(_) => break Some(StopReason::DeviceFailure),
        }
    };
    let rows = png.rows;
    let started = std::time::Instant::now();
    png.finish()?;
    timing.compress += started.elapsed();
    Ok((rows, stop, timing))
}

fn publish_no_clobber(temp: &Path, target: &Path) -> io::Result<()> {
    let linked = fs::hard_link(temp, target);
    let removed = fs::remove_file(temp);
    linked?;
    removed
}

fn write_manifest(
    report: &BatchReport,
    profile: &Profile,
    scanner: &str,
    status: &str,
    first: bool,
) -> io::Result<()> {
    let processing = profile
        .active_processing()
        .iter()
        .map(|name| json(name))
        .collect::<Vec<_>>()
        .join(", ");
    let sides = report
        .sides
        .iter()
        .map(|side| {
            format!(
                "    {{\"sheet\": {}, \"side\": {}, \"status\": {}, \"file\": {}, \"width\": {}, \"height\": {}, \"bytes\": {}, \"sha256\": {}, \"stop\": {}}}",
                side.sheet,
                json(side.side.tag()),
                json(if side.status == SideStatus::Complete { "complete" } else { "partial" }),
                json(&side.file),
                side.width,
                side.height,
                side.bytes,
                json(&side.sha256),
                side.stop.map_or("null".to_owned(), |stop| json(stop.name())),
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let text = format!(
        "{{\n  \"version\": 1,\n  \"batch\": {},\n  \"status\": {},\n  \"scanner\": {},\n  \"profile\": {},\n  \"dpi\": {},\n  \"color\": {},\n  \"duplex\": {},\n  \"max_length_mm\": {},\n  \"margin_mm\": {},\n  \"active_processing\": [{processing}],\n  \"sides\": [\n{sides}\n  ]\n}}\n",
        json(&report.batch),
        json(status),
        json(scanner),
        json(&profile.name),
        profile.dpi,
        json(profile.color.name()),
        profile.duplex,
        profile.max_length_mm,
        profile.margin_mm,
    );
    let name = report
        .manifest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = report.manifest.with_file_name(format!(".{name}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if first {
        publish_no_clobber(&temp, &report.manifest)
    } else {
        fs::rename(&temp, &report.manifest)
    }
}

fn json(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Streams a PNG whose height is only known after the last row.
struct PngStream {
    deflate: Deflater<IdatWriter>,
    width: u32,
    color: ColorMode,
    previous: Vec<u8>,
    filtered: Vec<u8>,
    rows: u32,
}

impl PngStream {
    fn new(file: File, width: u32, color: ColorMode, dpi: u32) -> io::Result<Self> {
        let mut out = BufWriter::new(file);
        out.write_all(b"\x89PNG\r\n\x1a\n")?;
        write_chunk(&mut out, b"IHDR", &ihdr(width, 1, color))?;
        let ppm = (f64::from(dpi) / 0.0254).round() as u32;
        let mut phys = [1; 9];
        phys[..4].copy_from_slice(&ppm.to_be_bytes());
        phys[4..8].copy_from_slice(&ppm.to_be_bytes());
        write_chunk(&mut out, b"pHYs", &phys)?;
        let stride = width as usize * color.channels();
        Ok(Self {
            deflate: Deflater::new(IdatWriter {
                out,
                buffer: Vec::with_capacity(IDAT_BYTES),
            })?,
            width,
            color,
            previous: vec![0; stride],
            filtered: vec![0; stride + 1],
            rows: 0,
        })
    }

    fn write_row(&mut self, row: &[u8]) -> io::Result<()> {
        let bpp = self.color.channels();
        let previous = &self.previous;
        let predict = |kind: u8, index: usize| -> u8 {
            let left = if index >= bpp { row[index - bpp] } else { 0 };
            let up = previous[index];
            let up_left = if index >= bpp {
                previous[index - bpp]
            } else {
                0
            };
            match kind {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                _ => paeth(left, up, up_left),
            }
        };
        let (mut best, mut best_score) = (0, u64::MAX);
        for kind in 0..5 {
            let score = (0..row.len())
                .map(|index| {
                    u64::from((row[index].wrapping_sub(predict(kind, index)) as i8).unsigned_abs())
                })
                .sum::<u64>();
            if score < best_score {
                (best, best_score) = (kind, score);
            }
        }
        self.filtered[0] = best;
        for index in 0..row.len() {
            self.filtered[index + 1] = row[index].wrapping_sub(predict(best, index));
        }
        self.deflate.write(&self.filtered)?;
        self.previous.copy_from_slice(row);
        self.rows += 1;
        Ok(())
    }
    fn finish(self) -> io::Result<()> {
        let (rows, width, color) = (self.rows, self.width, self.color);
        let mut writer = self.deflate.finish()?;
        writer.flush_chunk()?;
        write_chunk(&mut writer.out, b"IEND", &[])?;
        let mut file = writer
            .out
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        if rows > 0 {
            let data = ihdr(width, rows, color);
            let crc = crc32(crc32_update(crc32_update(0xffff_ffff, b"IHDR"), &data));
            file.seek(SeekFrom::Start(16))?;
            file.write_all(&data)?;
            file.write_all(&crc.to_be_bytes())?;
        }
        file.sync_all()
    }
}

struct IdatWriter {
    out: BufWriter<File>,
    buffer: Vec<u8>,
}

impl IdatWriter {
    fn flush_chunk(&mut self) -> io::Result<()> {
        if !self.buffer.is_empty() {
            write_chunk(&mut self.out, b"IDAT", &self.buffer)?;
            self.buffer.clear();
        }
        Ok(())
    }
}

impl Write for IdatWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let count = (IDAT_BYTES - self.buffer.len()).min(data.len());
        self.buffer.extend_from_slice(&data[..count]);
        if self.buffer.len() == IDAT_BYTES {
            self.flush_chunk()?;
        }
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_chunk()?;
        self.out.flush()
    }
}

fn ihdr(width: u32, height: u32, color: ColorMode) -> [u8; 13] {
    let mut data = [0; 13];
    data[..4].copy_from_slice(&width.to_be_bytes());
    data[4..8].copy_from_slice(&height.to_be_bytes());
    data[8] = 8;
    data[9] = color.png_type();
    data
}

fn write_chunk<W: Write>(out: &mut W, kind: &[u8; 4], data: &[u8]) -> io::Result<()> {
    out.write_all(&(data.len() as u32).to_be_bytes())?;
    out.write_all(kind)?;
    out.write_all(data)?;
    let crc = crc32(crc32_update(crc32_update(0xffff_ffff, kind), data));
    out.write_all(&crc.to_be_bytes())
}

fn crc32(state: u32) -> u32 {
    !state
}

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0; 256];
        let mut index = 0;
        while index < 256 {
            let mut value = index as u32;
            let mut bit = 0;
            while bit < 8 {
                value = if value & 1 == 1 {
                    0xedb8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                };
                bit += 1;
            }
            table[index] = value;
            index += 1;
        }
        table
    };
    for byte in data {
        crc = TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc
}

fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(up) - i16::from(up_left);
    let (a, b, c) = (
        (estimate - i16::from(left)).abs(),
        (estimate - i16::from(up)).abs(),
        (estimate - i16::from(up_left)).abs(),
    );
    if a <= b && a <= c {
        left
    } else if b <= c {
        up
    } else {
        up_left
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const WINDOW: usize = 32 * 1024;
const MAX_MATCH: usize = 258;
const HASH_SIZE: usize = 1 << 15;
const MAX_CHAIN: usize = 48;
const SLIDE_AT: usize = 2 * 1024 * 1024;
const BLOCK_TOKENS: usize = 128 * 1024;
const MATCH_TOKEN: u32 = 0x8000_0000;

/// A streaming zlib encoder using LZ77 and dynamic Huffman blocks.
struct Deflater<W: Write> {
    out: W,
    bits: u64,
    bit_count: u32,
    data: Vec<u8>,
    position: usize,
    head: Vec<i32>,
    previous: Vec<i32>,
    tokens: Vec<u32>,
    adler_a: u32,
    adler_b: u32,
}

impl<W: Write> Deflater<W> {
    fn new(mut out: W) -> io::Result<Self> {
        out.write_all(&[0x78, 0x9c])?;
        Ok(Self {
            out,
            bits: 0,
            bit_count: 0,
            data: Vec::with_capacity(SLIDE_AT + MAX_MATCH),
            position: 0,
            head: vec![-1; HASH_SIZE],
            previous: vec![-1; WINDOW],
            tokens: Vec::with_capacity(BLOCK_TOKENS),
            adler_a: 1,
            adler_b: 0,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        for chunk in bytes.chunks(5552) {
            for byte in chunk {
                self.adler_a += u32::from(*byte);
                self.adler_b += self.adler_a;
            }
            self.adler_a %= 65521;
            self.adler_b %= 65521;
        }
        self.data.extend_from_slice(bytes);
        self.compress(false)
    }

    fn finish(mut self) -> io::Result<W> {
        self.compress(true)?;
        self.flush_block(true)?;
        if self.bit_count > 0 {
            self.put_bits(0, 8 - self.bit_count)?;
        }
        let adler = (self.adler_b << 16) | self.adler_a;
        self.out.write_all(&adler.to_be_bytes())?;
        Ok(self.out)
    }

    fn compress(&mut self, last: bool) -> io::Result<()> {
        let reserve = if last { 0 } else { MAX_MATCH + 3 };
        while self.position + reserve < self.data.len() {
            let (length, distance) = self.longest_match();
            if length >= 3 {
                self.tokens
                    .push(MATCH_TOKEN | (length as u32) << 16 | distance as u32);
                for offset in 0..length {
                    self.insert(self.position + offset);
                }
                self.position += length;
            } else {
                self.tokens.push(u32::from(self.data[self.position]));
                self.insert(self.position);
                self.position += 1;
            }
            if self.tokens.len() >= BLOCK_TOKENS {
                self.flush_block(false)?;
            }
        }
        if self.position >= SLIDE_AT {
            self.slide();
        }
        Ok(())
    }

    fn hash(&self, at: usize) -> usize {
        let value = u32::from(self.data[at])
            | u32::from(self.data[at + 1]) << 8
            | u32::from(self.data[at + 2]) << 16;
        (value.wrapping_mul(0x9e37_79b1) >> 17) as usize
    }

    fn insert(&mut self, at: usize) {
        if at + 3 <= self.data.len() {
            let hash = self.hash(at);
            self.previous[at & (WINDOW - 1)] = self.head[hash];
            self.head[hash] = at as i32;
        }
    }

    fn longest_match(&self) -> (usize, usize) {
        let at = self.position;
        if at + 3 > self.data.len() {
            return (0, 0);
        }
        let limit = MAX_MATCH.min(self.data.len() - at);
        let (mut best, mut best_distance) = (0, 0);
        let mut candidate = self.head[self.hash(at)];
        for _ in 0..MAX_CHAIN {
            if candidate < 0 {
                break;
            }
            let start = candidate as usize;
            let distance = at - start;
            if distance == 0 || distance > WINDOW {
                break;
            }
            let length = (0..limit)
                .take_while(|i| self.data[start + i] == self.data[at + i])
                .count();
            if length > best {
                (best, best_distance) = (length, distance);
                if best == limit {
                    break;
                }
            }
            candidate = self.previous[start & (WINDOW - 1)];
        }
        (best, best_distance)
    }

    fn slide(&mut self) {
        let cut = (self.position - WINDOW) & !(WINDOW - 1);
        self.data.drain(..cut);
        self.position -= cut;
        let shift = cut as i32;
        for entry in self.head.iter_mut().chain(self.previous.iter_mut()) {
            *entry = if *entry >= shift { *entry - shift } else { -1 };
        }
    }

    fn put_bits(&mut self, value: u32, count: u32) -> io::Result<()> {
        self.bits |= u64::from(value) << self.bit_count;
        self.bit_count += count;
        while self.bit_count >= 8 {
            self.out.write_all(&[self.bits as u8])?;
            self.bits >>= 8;
            self.bit_count -= 8;
        }
        Ok(())
    }

    fn put_code(&mut self, code: u32, length: u8) -> io::Result<()> {
        self.put_bits(
            code.reverse_bits() >> (32 - u32::from(length)),
            u32::from(length),
        )
    }

    fn flush_block(&mut self, last: bool) -> io::Result<()> {
        let tokens = std::mem::take(&mut self.tokens);
        let mut literal_counts = [0u32; 286];
        let mut distance_counts = [0u32; 30];
        literal_counts[256] = 1;
        for token in &tokens {
            if token & MATCH_TOKEN == 0 {
                literal_counts[*token as usize] += 1;
            } else {
                let (length, distance) = token_parts(*token);
                literal_counts[257 + length_index(length)] += 1;
                distance_counts[distance_index(distance)] += 1;
            }
        }
        let literal_lengths = huffman_lengths(&literal_counts, 15);
        let distance_lengths = huffman_lengths(&distance_counts, 15);
        let literal_used = literal_lengths
            .iter()
            .rposition(|length| *length != 0)
            .map_or(257, |index| (index + 1).max(257));
        let distance_used = distance_lengths
            .iter()
            .rposition(|length| *length != 0)
            .map_or(1, |index| index + 1);
        let mut all_lengths = literal_lengths[..literal_used].to_vec();
        all_lengths.extend_from_slice(&distance_lengths[..distance_used]);
        let encoded = run_length_encode(&all_lengths);
        let mut code_counts = [0u32; 19];
        for (symbol, _, _) in &encoded {
            code_counts[usize::from(*symbol)] += 1;
        }
        let code_lengths = huffman_lengths(&code_counts, 7);
        let code_codes = canonical_codes(&code_lengths);
        const ORDER: [usize; 19] = [
            16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
        ];
        let code_used = ORDER
            .iter()
            .rposition(|symbol| code_lengths[*symbol] != 0)
            .map_or(4, |index| (index + 1).max(4));

        self.put_bits(u32::from(last), 1)?;
        self.put_bits(0b10, 2)?;
        self.put_bits((literal_used - 257) as u32, 5)?;
        self.put_bits((distance_used - 1) as u32, 5)?;
        self.put_bits((code_used - 4) as u32, 4)?;
        for symbol in &ORDER[..code_used] {
            self.put_bits(u32::from(code_lengths[*symbol]), 3)?;
        }
        for (symbol, extra, extra_bits) in encoded {
            let index = usize::from(symbol);
            self.put_code(code_codes[index], code_lengths[index])?;
            self.put_bits(u32::from(extra), u32::from(extra_bits))?;
        }

        let literal_codes = canonical_codes(&literal_lengths);
        let distance_codes = canonical_codes(&distance_lengths);
        for token in &tokens {
            if token & MATCH_TOKEN == 0 {
                let symbol = *token as usize;
                self.put_code(literal_codes[symbol], literal_lengths[symbol])?;
            } else {
                let (length, distance) = token_parts(*token);
                let index = length_index(length);
                self.put_code(literal_codes[257 + index], literal_lengths[257 + index])?;
                self.put_bits(
                    (length - usize::from(LEN_BASE[index])) as u32,
                    u32::from(LEN_EXTRA[index]),
                )?;
                let index = distance_index(distance);
                self.put_code(distance_codes[index], distance_lengths[index])?;
                self.put_bits(
                    (distance - usize::from(DIST_BASE[index])) as u32,
                    u32::from(DIST_EXTRA[index]),
                )?;
            }
        }
        self.put_code(literal_codes[256], literal_lengths[256])?;
        self.tokens = tokens;
        self.tokens.clear();
        Ok(())
    }
}

fn token_parts(token: u32) -> (usize, usize) {
    (((token >> 16) & 0x1ff) as usize, (token & 0xffff) as usize)
}

fn length_index(length: usize) -> usize {
    LEN_BASE
        .iter()
        .rposition(|base| usize::from(*base) <= length)
        .unwrap_or(0)
}

fn distance_index(distance: usize) -> usize {
    DIST_BASE
        .iter()
        .rposition(|base| usize::from(*base) <= distance)
        .unwrap_or(0)
}

/// Builds complete Huffman code lengths no longer than `limit`.
fn huffman_lengths(counts: &[u32], limit: u8) -> Vec<u8> {
    let mut weights = counts.to_vec();
    let mut used = weights.iter().filter(|weight| **weight > 0).count();
    for weight in weights.iter_mut() {
        if used >= 2 {
            break;
        }
        if *weight == 0 {
            *weight = 1;
            used += 1;
        }
    }
    loop {
        let lengths = huffman_tree_lengths(&weights);
        if lengths.iter().all(|length| *length <= limit) {
            return lengths;
        }
        for weight in weights.iter_mut().filter(|weight| **weight > 0) {
            *weight = (*weight >> 1).max(1);
        }
    }
}

fn huffman_tree_lengths(weights: &[u32]) -> Vec<u8> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let mut parents = vec![usize::MAX; weights.len()];
    let mut heap = BinaryHeap::new();
    for (index, weight) in weights.iter().enumerate() {
        if *weight > 0 {
            heap.push(Reverse((u64::from(*weight), index)));
        }
    }
    while heap.len() > 1 {
        let Reverse((first_weight, first)) = heap.pop().expect("two nodes");
        let Reverse((second_weight, second)) = heap.pop().expect("two nodes");
        let parent = parents.len();
        parents.push(usize::MAX);
        parents[first] = parent;
        parents[second] = parent;
        heap.push(Reverse((first_weight + second_weight, parent)));
    }
    weights
        .iter()
        .enumerate()
        .map(|(index, weight)| {
            if *weight == 0 {
                return 0;
            }
            let (mut node, mut depth) = (index, 0u8);
            while parents[node] != usize::MAX {
                node = parents[node];
                depth = depth.saturating_add(1);
            }
            depth
        })
        .collect()
}

fn canonical_codes(lengths: &[u8]) -> Vec<u32> {
    let mut counts = [0u32; 16];
    for length in lengths {
        counts[usize::from(*length)] += 1;
    }
    counts[0] = 0;
    let mut next = [0u32; 16];
    let mut code = 0;
    for bits in 1..16 {
        code = (code + counts[bits - 1]) << 1;
        next[bits] = code;
    }
    lengths
        .iter()
        .map(|length| {
            if *length == 0 {
                return 0;
            }
            let value = next[usize::from(*length)];
            next[usize::from(*length)] += 1;
            value
        })
        .collect()
}

fn run_length_encode(lengths: &[u8]) -> Vec<(u8, u8, u8)> {
    let mut encoded = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let run = lengths[index..]
            .iter()
            .take_while(|length| **length == value)
            .count();
        let mut left = run;
        if value == 0 {
            while left >= 11 {
                let count = left.min(138);
                encoded.push((18, (count - 11) as u8, 7));
                left -= count;
            }
            if left >= 3 {
                encoded.push((17, (left - 3) as u8, 3));
                left = 0;
            }
        } else {
            encoded.push((value, 0, 0));
            left -= 1;
            while left >= 3 {
                let count = left.min(6);
                encoded.push((16, (count - 3) as u8, 2));
                left -= count;
            }
        }
        encoded.extend(std::iter::repeat_n((value, 0, 0), left));
        index += run;
    }
    encoded
}

fn sha256_file(path: &Path) -> io::Result<(String, u64)> {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut block = |chunk: &[u8]| {
        let mut w = [0u32; 64];
        for (index, word) in chunk.chunks_exact(4).enumerate() {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = state;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let choice = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let majority = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(majority);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (slot, value) in state.iter_mut().zip(v) {
            *slot = slot.wrapping_add(value);
        }
    };
    let mut file = File::open(path)?;
    let mut buffer = vec![0; 64 * 1024];
    let (mut pending, mut total) = (Vec::with_capacity(128), 0u64);
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        let mut input = &buffer[..count];
        if !pending.is_empty() {
            let take = (64 - pending.len()).min(input.len());
            pending.extend_from_slice(&input[..take]);
            input = &input[take..];
            if pending.len() == 64 {
                block(&pending);
                pending.clear();
            }
        }
        let whole = input.len() / 64 * 64;
        input[..whole].chunks_exact(64).for_each(&mut block);
        pending.extend_from_slice(&input[whole..]);
    }
    pending.push(0x80);
    while pending.len() % 64 != 56 {
        pending.push(0);
    }
    pending.extend_from_slice(&(total * 8).to_be_bytes());
    pending.chunks_exact(64).for_each(&mut block);
    Ok((
        state.iter().map(|word| format!("{word:08x}")).collect(),
        total,
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedSheet {
    pub front: Option<String>,
    pub back: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairReport {
    pub sheets: Vec<PairedSheet>,
    pub orphan_backs: Vec<String>,
    pub ignored: usize,
}

/// Pairs existing CapturePerfect scans without changing them.
pub fn pair_capture_perfect(directory: &Path) -> io::Result<PairReport> {
    let mut names = Vec::new();
    let mut ignored = 0;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file = entry.file_name().to_string_lossy().into_owned();
        match capture_name(&file) {
            Some((stamp, side)) => names.push((stamp, file, side)),
            None => ignored += 1,
        }
    }
    names.sort();
    let (mut sheets, mut orphan_backs) = (Vec::<PairedSheet>::new(), Vec::new());
    let mut open = None;
    for (_, file, side) in names {
        if side == "Fron" {
            sheets.push(PairedSheet {
                front: Some(file),
                back: None,
            });
            open = Some(sheets.len() - 1);
        } else if let Some(index) = open.take() {
            sheets[index].back = Some(file);
        } else {
            orphan_backs.push(file.clone());
            sheets.push(PairedSheet {
                front: None,
                back: Some(file),
            });
        }
    }
    Ok(PairReport {
        sheets,
        orphan_backs,
        ignored,
    })
}

fn capture_name(file: &str) -> Option<(String, String)> {
    let (stem, extension) = file.rsplit_once('.')?;
    if !["png", "jpg", "jpeg", "tif", "tiff", "bmp"]
        .contains(&extension.to_ascii_lowercase().as_str())
    {
        return None;
    }
    let parts = stem.rsplitn(6, '_').collect::<Vec<_>>();
    let [counter, _dropout, dpi, side, stamp, _prefix] = parts.as_slice() else {
        return None;
    };
    let digits = |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    (stamp.len() == 17
        && digits(stamp)
        && digits(dpi)
        && digits(counter)
        && ["Fron", "Back"].contains(side))
    .then(|| ((*stamp).to_owned(), (*side).to_owned()))
}

#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

type Handle = *mut core::ffi::c_void;
type DsmEntry = unsafe extern "system" fn(
    *mut TwIdentity,
    *mut TwIdentity,
    u32,
    u16,
    u16,
    *mut core::ffi::c_void,
) -> u16;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocalTime(time: *mut SystemTime);
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> Handle;
    fn FreeLibrary(module: Handle) -> i32;
    fn GlobalLock(memory: Handle) -> *mut u8;
    fn GlobalUnlock(memory: Handle) -> i32;
    fn GlobalFree(memory: Handle) -> Handle;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        param: Handle,
    ) -> Handle;
    fn DestroyWindow(window: Handle) -> i32;
}

fn local_stamp() -> String {
    let mut time = SystemTime::default();
    // SAFETY: `time` is a valid writable SYSTEMTIME for the duration of the call.
    unsafe { GetLocalTime(&mut time) };
    format!(
        "{:04}{:02}{:02}{:02}{:02}{:02}{:03}",
        time.year, time.month, time.day, time.hour, time.minute, time.second, time.milliseconds
    )
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

const DG_CONTROL: u32 = 1;
const DG_IMAGE: u32 = 2;
const DF_APP2: u32 = 0x2000_0000;
const DAT_CAPABILITY: u16 = 1;
const DAT_IDENTITY: u16 = 3;
const DAT_PARENT: u16 = 4;
const MSG_GET: u16 = 1;
const MSG_GETCURRENT: u16 = 2;
const MSG_GETDEFAULT: u16 = 3;
const MSG_GETFIRST: u16 = 4;
const MSG_GETNEXT: u16 = 5;
const MSG_OPENDSM: u16 = 0x0301;
const MSG_CLOSEDSM: u16 = 0x0302;
const MSG_OPENDS: u16 = 0x0401;
const MSG_CLOSEDS: u16 = 0x0402;
const MSG_USERSELECT: u16 = 0x0403;
const TWRC_SUCCESS: u16 = 0;
const CAP_SUPPORTEDCAPS: u16 = 0x1005;
const KNOWN_CAPS: [(u16, &str); 18] = [
    (0x0100, "ICAP_COMPRESSION"),
    (0x0101, "ICAP_PIXELTYPE"),
    (0x0103, "ICAP_XFERMECH"),
    (0x1002, "CAP_FEEDERENABLED"),
    (0x1003, "CAP_FEEDERLOADED"),
    (0x1007, "CAP_AUTOFEED"),
    (0x100D, "CAP_PAPERDETECTABLE"),
    (0x100E, "CAP_UICONTROLLABLE"),
    (0x100F, "CAP_DEVICEONLINE"),
    (0x1010, "CAP_AUTOSCAN"),
    (0x1012, "CAP_DUPLEX"),
    (0x1013, "CAP_DUPLEXENABLED"),
    (0x1101, "ICAP_BRIGHTNESS"),
    (0x1103, "ICAP_CONTRAST"),
    (0x1118, "ICAP_XRESOLUTION"),
    (0x1119, "ICAP_YRESOLUTION"),
    (0x112B, "ICAP_BITDEPTH"),
    (0x112D, "ICAP_UNDEFINEDIMAGESIZE"),
];

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct TwVersion {
    major: u16,
    minor: u16,
    language: u16,
    country: u16,
    info: [u8; 34],
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct TwIdentity {
    id: u32,
    version: TwVersion,
    protocol_major: u16,
    protocol_minor: u16,
    supported_groups: u32,
    manufacturer: [u8; 34],
    product_family: [u8; 34],
    product_name: [u8; 34],
}

#[repr(C, packed(2))]
struct TwCapability {
    cap: u16,
    container_type: u16,
    container: Handle,
}

fn tw_text(value: &str) -> [u8; 34] {
    let mut out = [0; 34];
    let bytes = value.as_bytes();
    let count = bytes.len().min(33);
    out[..count].copy_from_slice(&bytes[..count]);
    out
}

fn tw_string(value: &[u8; 34]) -> String {
    let end = value
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(value.len());
    String::from_utf8_lossy(&value[..end]).into_owned()
}

fn empty_identity() -> TwIdentity {
    TwIdentity {
        id: 0,
        version: TwVersion {
            major: 0,
            minor: 0,
            language: 0,
            country: 0,
            info: [0; 34],
        },
        protocol_major: 0,
        protocol_minor: 0,
        supported_groups: 0,
        manufacturer: [0; 34],
        product_family: [0; 34],
        product_name: [0; 34],
    }
}

fn app_identity() -> TwIdentity {
    TwIdentity {
        id: 0,
        version: TwVersion {
            major: 0,
            minor: 1,
            language: 2,
            country: 1,
            info: tw_text(VERSION),
        },
        protocol_major: 2,
        protocol_minor: 3,
        supported_groups: DG_CONTROL | DG_IMAGE | DF_APP2,
        manufacturer: tw_text("OwnScanner"),
        product_family: tw_text("OwnScanner"),
        product_name: tw_text("OwnScanner"),
    }
}

fn twain_item_size(item_type: u16) -> usize {
    match item_type {
        0 | 3 => 1,
        1 | 4 | 6 => 2,
        _ => 4,
    }
}

/// SAFETY: `at` must point to a readable item of `item_type`.
unsafe fn twain_item(at: *const u8, item_type: u16) -> String {
    // SAFETY: the caller guarantees the item is readable; unaligned reads match TWAIN packing.
    unsafe {
        match item_type {
            0 => (at as *const i8).read_unaligned().to_string(),
            1 => (at as *const i16).read_unaligned().to_string(),
            2 => (at as *const i32).read_unaligned().to_string(),
            3 => at.read_unaligned().to_string(),
            4 => (at as *const u16).read_unaligned().to_string(),
            5 => (at as *const u32).read_unaligned().to_string(),
            6 => ((at as *const u16).read_unaligned() != 0).to_string(),
            7 => {
                let whole = (at as *const i16).read_unaligned();
                let fraction = (at.add(2) as *const u16).read_unaligned();
                format!("{:.3}", f64::from(whole) + f64::from(fraction) / 65536.0)
            }
            other => format!("<type {other}>"),
        }
    }
}

/// SAFETY: `at` must point to a locked TWAIN container of `container_type`.
unsafe fn twain_values(container_type: u16, at: *const u8) -> (Vec<String>, String) {
    // SAFETY: offsets follow the packed TWAIN container layouts.
    unsafe {
        let item_type = (at as *const u16).read_unaligned();
        let size = twain_item_size(item_type);
        match container_type {
            3 | 4 => {
                let count = (at.add(2) as *const u32).read_unaligned() as usize;
                let first = if container_type == 3 { 6 } else { 14 };
                let values = (0..count.min(4096))
                    .map(|index| twain_item(at.add(first + index * size), item_type))
                    .collect();
                let current = if container_type == 4 {
                    let index = (at.add(6) as *const u32).read_unaligned() as usize;
                    if index < count {
                        twain_item(at.add(first + index * size), item_type)
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };
                (values, current)
            }
            5 => {
                let value = twain_item(at.add(2), item_type);
                (vec![value.clone()], value)
            }
            6 => {
                let part = |offset| twain_item(at.add(offset), item_type);
                (
                    vec![format!("{}..{} step {}", part(2), part(6), part(10))],
                    part(18),
                )
            }
            other => (vec![format!("<container {other}>")], String::new()),
        }
    }
}

/// Reads scanner capabilities. It opens the driver without enabling a scan or feeding paper.
pub fn twain_probe(
    out: &mut dyn Write,
    preferred: Option<&str>,
    list_only: bool,
) -> io::Result<i32> {
    // SAFETY: all pointers passed to Win32 and TWAIN outlive their calls; opened
    // handles are closed below in reverse order.
    unsafe {
        let module = LoadLibraryW(wide("TWAINDSM.dll").as_ptr());
        if module.is_null() {
            return Err(io::Error::last_os_error());
        }
        let address = GetProcAddress(module, c"DSM_Entry".as_ptr().cast());
        if address.is_null() {
            FreeLibrary(module);
            return Err(io::Error::other("TWAINDSM.dll has no DSM_Entry"));
        }
        let entry: DsmEntry = std::mem::transmute(address);
        let mut window = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("OwnScanner").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if window.is_null() {
            FreeLibrary(module);
            return Err(io::Error::last_os_error());
        }
        let mut app = app_identity();
        let parent = (&raw mut window).cast();
        let opened = entry(
            &mut app,
            std::ptr::null_mut(),
            DG_CONTROL,
            DAT_PARENT,
            MSG_OPENDSM,
            parent,
        );
        if opened != TWRC_SUCCESS {
            DestroyWindow(window);
            FreeLibrary(module);
            return Err(io::Error::other(format!(
                "cannot open TWAIN manager: {opened}"
            )));
        }
        let (sources, system_default) = twain_sources(entry, &mut app);
        let names = sources
            .iter()
            .map(|source| tw_string(&{ source.product_name }))
            .collect::<Vec<_>>();
        for name in &names {
            let marker = if system_default.as_deref() == Some(name) {
                " (system default)"
            } else {
                ""
            };
            writeln!(out, "source: {name}{marker}")?;
        }
        let chosen = match choose_source(&names, preferred, system_default.as_deref()) {
            SourceChoice::Use { name, reason } => {
                writeln!(out, "selected: {name} ({reason})")?;
                sources
                    .iter()
                    .copied()
                    .find(|source| tw_string(&{ source.product_name }) == name)
            }
            SourceChoice::Ask { reason } => {
                writeln!(out, "selection needed: {reason}")?;
                None
            }
            SourceChoice::None => {
                writeln!(out, "no TWAIN scanners found")?;
                None
            }
        };
        let chosen = if list_only { None } else { chosen };
        let mut code = i32::from(!list_only);
        if let Some(mut scanner) = chosen {
            let status = entry(
                &mut app,
                std::ptr::null_mut(),
                DG_CONTROL,
                DAT_IDENTITY,
                MSG_OPENDS,
                (&raw mut scanner).cast(),
            );
            if status == TWRC_SUCCESS {
                let mut read = |cap: u16, message: u16| -> Option<(Vec<String>, String)> {
                    let mut request = TwCapability {
                        cap,
                        container_type: 0xffff,
                        container: std::ptr::null_mut(),
                    };
                    if entry(
                        &mut app,
                        &mut scanner,
                        DG_CONTROL,
                        DAT_CAPABILITY,
                        message,
                        (&raw mut request).cast(),
                    ) != TWRC_SUCCESS
                    {
                        return None;
                    }
                    let handle = request.container;
                    let memory = GlobalLock(handle);
                    let values =
                        (!memory.is_null()).then(|| twain_values(request.container_type, memory));
                    GlobalUnlock(handle);
                    GlobalFree(handle);
                    values
                };
                let supported = read(CAP_SUPPORTEDCAPS, MSG_GET)
                    .map(|(values, _)| values)
                    .unwrap_or_default();
                let numbers = supported
                    .iter()
                    .filter_map(|value| value.parse::<u16>().ok())
                    .collect::<Vec<_>>();
                writeln!(out, "capabilities: {}", numbers.len())?;
                for (cap, name) in KNOWN_CAPS {
                    if numbers.contains(&cap) {
                        let current = read(cap, MSG_GETCURRENT)
                            .map_or("unavailable".to_owned(), |(_, value)| value);
                        let options = read(cap, MSG_GET).map_or(String::new(), |(values, _)| {
                            values
                                .iter()
                                .take(16)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(",")
                        });
                        writeln!(
                            out,
                            "{name} [{cap:#06x}] current={current} options={options}"
                        )?;
                    }
                }
                let vendor = numbers
                    .iter()
                    .filter(|cap| **cap >= 0x8000)
                    .map(|cap| format!("{cap:#06x}"))
                    .collect::<Vec<_>>();
                writeln!(out, "vendor capabilities: {}", vendor.join(" "))?;
                for cap in numbers.iter().copied().filter(|cap| *cap >= 0x8000) {
                    let current = read(cap, MSG_GETCURRENT)
                        .map_or("unavailable".to_owned(), |(_, value)| value);
                    let options = read(cap, MSG_GET).map_or(String::new(), |(values, _)| {
                        values
                            .iter()
                            .take(12)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(",")
                    });
                    writeln!(
                        out,
                        "vendor [{cap:#06x}] current={current} options={options}"
                    )?;
                }
                for cap in numbers.iter().copied().filter(|cap| {
                    *cap < 0x8000 && !KNOWN_CAPS.iter().any(|(known, _)| known == cap)
                }) {
                    let current = read(cap, MSG_GETCURRENT)
                        .map_or("unavailable".to_owned(), |(_, value)| value);
                    let options = read(cap, MSG_GET).map_or(String::new(), |(values, _)| {
                        values
                            .iter()
                            .take(48)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(",")
                    });
                    writeln!(
                        out,
                        "standard [{cap:#06x}] current={current} options={options}"
                    )?;
                }
                for line in transfer_mode_report(entry, &mut app, &mut scanner) {
                    writeln!(out, "{line}")?;
                }
                let long_cap = device_profile(
                    &tw_string(&{ scanner.manufacturer }),
                    &tw_string(&{ scanner.product_name }),
                )
                .and_then(|device| device.long_document_cap);
                if let Some(cap) = long_cap {
                    let mode = apply_long_document_mode(entry, &mut app, &mut scanner, cap, 5588);
                    let height = twain_get(entry, &mut app, &mut scanner, 0x1112, MSG_GETCURRENT)
                        .map_or("unavailable".to_owned(), |(_, _, value)| value);
                    writeln!(
                        out,
                        "long-document test: mode={} driver max height in={height}",
                        mode.as_deref().unwrap_or("rejected")
                    )?;
                }
                entry(
                    &mut app,
                    std::ptr::null_mut(),
                    DG_CONTROL,
                    DAT_IDENTITY,
                    MSG_CLOSEDS,
                    (&raw mut scanner).cast(),
                );
                code = 0;
            } else {
                writeln!(out, "cannot open scanner: TWAIN return code {status}")?;
            }
        } else {
            if !list_only {
                writeln!(out, "no scanner selected")?;
            }
        }
        entry(
            &mut app,
            std::ptr::null_mut(),
            DG_CONTROL,
            DAT_PARENT,
            MSG_CLOSEDSM,
            parent,
        );
        DestroyWindow(window);
        FreeLibrary(module);
        Ok(code)
    }
}

use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

const DAT_EVENT: u16 = 2;
const DAT_PENDINGXFERS: u16 = 5;
const DAT_SETUPMEMXFER: u16 = 6;
const DAT_STATUS: u16 = 8;
const DAT_USERINTERFACE: u16 = 9;
const DAT_IMAGEINFO: u16 = 0x0101;
const DAT_IMAGEMEMXFER: u16 = 0x0103;
const MSG_SET: u16 = 6;
const MSG_RESET: u16 = 7;
const MSG_XFERREADY: u16 = 0x0101;
const MSG_CLOSEDSREQ: u16 = 0x0102;
const MSG_CLOSEDSOK: u16 = 0x0103;
const MSG_DISABLEDS: u16 = 0x0501;
const MSG_ENABLEDS: u16 = 0x0502;
const MSG_PROCESSEVENT: u16 = 0x0601;
const MSG_ENDXFER: u16 = 0x0701;
const MSG_STOPFEEDER: u16 = 0x0702;
const PAUSE_GRACE_POLLS: u32 = 600;
const MSG_DEVICEEVENT: u16 = 0x0104;
const DAT_DEVICEEVENT: u16 = 0x000d;
const CAP_DEVICEEVENT: u16 = 0x1022;
const TWDE_DEVICEOFFLINE: u16 = 7;
const TWDE_DEVICEREADY: u16 = 8;
const TWDE_DEVICEREMOVED: u16 = 9;
const TWDE_PAPERDOUBLEFEED: u16 = 12;
const TWDE_PAPERJAM: u16 = 13;
const TWRC_CHECKSTATUS: u16 = 2;
const TWRC_CANCEL: u16 = 3;
const TWRC_DSEVENT: u16 = 4;
const TWRC_XFERDONE: u16 = 6;
const TWTY_INT16: u16 = 1;
const TWTY_INT32: u16 = 2;
const TWTY_UINT16: u16 = 4;
const TWTY_BOOL: u16 = 6;
const TWTY_FIX32: u16 = 7;
const CAP_XFERCOUNT: u16 = 0x0001;
const ICAP_COMPRESSION: u16 = 0x0100;
const ICAP_PIXELTYPE: u16 = 0x0101;
const ICAP_UNITS: u16 = 0x0102;
const ICAP_XFERMECH: u16 = 0x0103;
const CAP_FEEDERENABLED: u16 = 0x1002;
const CAP_FEEDERLOADED: u16 = 0x1003;
const CAP_AUTOFEED: u16 = 0x1007;
const CAP_INDICATORS: u16 = 0x100b;
const CAP_AUTOSCAN: u16 = 0x1010;
const CAP_DUPLEXENABLED: u16 = 0x1013;
const ICAP_XRESOLUTION: u16 = 0x1118;
const ICAP_YRESOLUTION: u16 = 0x1119;
const ICAP_PIXELFLAVOR: u16 = 0x111f;
const ICAP_PLANARCHUNKY: u16 = 0x1120;
const ICAP_BITDEPTH: u16 = 0x112b;
const ICAP_UNDEFINEDIMAGESIZE: u16 = 0x112d;
const ICAP_EXTIMAGEINFO: u16 = 0x112f;
const ICAP_PHYSICALWIDTH: u16 = 0x1111;
const ICAP_PHYSICALHEIGHT: u16 = 0x1112;
const ICAP_SUPPORTEDSIZES: u16 = 0x1122;
const ICAP_AUTOSIZE: u16 = 0x1156;
const TWSS_NONE: u32 = 0;
const DAT_EXTIMAGEINFO: u16 = 0x010b;
const EXT_INFO_FIRST: u16 = 0x1200;
const EXT_INFO_COUNT: usize = 0x53;
const ICAP_AUTODISCARDBLANKPAGES: u16 = 0x1134;
const ICAP_AUTOMATICBORDERDETECTION: u16 = 0x1150;
const ICAP_AUTOMATICDESKEW: u16 = 0x1151;
const ICAP_AUTOMATICROTATE: u16 = 0x1152;
/// Optional model knowledge. Scanning must work when no profile matches.
struct DeviceProfile {
    name: &'static str,
    matches: fn(&str, &str) -> bool,
    lock_holders: &'static [(&'static str, &'static str)],
    jam_help: Option<&'static str>,
    long_document_cap: Option<u16>,
}

impl DeviceProfile {
    fn help(&self, reason: StopReason) -> Option<&'static str> {
        match reason {
            StopReason::Jam => self.jam_help,
            _ => None,
        }
    }
}

fn is_canon_dr_m(manufacturer: &str, product: &str) -> bool {
    product.contains("DR-M")
        && format!("{manufacturer} {product}")
            .to_ascii_lowercase()
            .contains("canon")
}

// Add new model support here; keep vendor details out of the generic flow.
static DEVICE_PROFILES: [DeviceProfile; 1] = [DeviceProfile {
    name: "Canon DR-M",
    matches: is_canon_dr_m,
    lock_holders: &[
        ("Cappe31.exe", "CapturePerfect"),
        ("CaptureOnTouch.exe", "CaptureOnTouch"),
        ("CaptureOnTouchPro.exe", "CaptureOnTouch Pro"),
    ],
    jam_help: Some(
        "Paper is stuck. Remove the feeder stack, pull the OPEN lever, open the front unit, remove the stuck paper, close the unit, then reload.",
    ),
    long_document_cap: Some(0x814d),
}];

/// Chooses the shortest driver length mode that covers the requested document length.
fn long_document_mode(options: &[u32], wanted_mm: u32) -> Option<u32> {
    const STANDARD_MM: u32 = 356;
    if wanted_mm <= STANDARD_MM && options.contains(&0) {
        return Some(0);
    }
    options
        .iter()
        .copied()
        .filter(|mode| *mode >= wanted_mm)
        .min()
        .or_else(|| options.iter().copied().max())
}

fn twain_get(
    entry: DsmEntry,
    app: &mut TwIdentity,
    source: &mut TwIdentity,
    cap: u16,
    message: u16,
) -> Option<(u16, Vec<String>, String)> {
    let mut request = TwCapability {
        cap,
        container_type: 0xffff,
        container: std::ptr::null_mut(),
    };
    // SAFETY: the identities and capability request are valid; the returned container
    // is locked for parsing and released here.
    unsafe {
        if entry(
            app,
            source,
            DG_CONTROL,
            DAT_CAPABILITY,
            message,
            (&raw mut request).cast(),
        ) != TWRC_SUCCESS
        {
            return None;
        }
        let handle = request.container;
        let memory = GlobalLock(handle);
        let parsed = (!memory.is_null()).then(|| {
            let item_type = (memory as *const u16).read_unaligned();
            let (values, current) = twain_values(request.container_type, memory);
            (item_type, values, current)
        });
        GlobalUnlock(handle);
        GlobalFree(handle);
        parsed
    }
}

fn twain_set(
    entry: DsmEntry,
    app: &mut TwIdentity,
    source: &mut TwIdentity,
    cap: u16,
    item_type: u16,
    value: u32,
) -> bool {
    // SAFETY: the allocated block holds a packed TW_ONEVALUE and is unlocked before use.
    unsafe {
        let handle = GlobalAlloc(0x42, 16);
        if handle.is_null() {
            return false;
        }
        let memory = GlobalLock(handle);
        if memory.is_null() {
            GlobalFree(handle);
            return false;
        }
        (memory as *mut u16).write_unaligned(item_type);
        (memory.add(2) as *mut u32).write_unaligned(value);
        GlobalUnlock(handle);
        let mut request = TwCapability {
            cap,
            container_type: 5,
            container: handle,
        };
        let result = entry(
            app,
            source,
            DG_CONTROL,
            DAT_CAPABILITY,
            MSG_SET,
            (&raw mut request).cast(),
        );
        GlobalFree(handle);
        result == TWRC_SUCCESS || result == TWRC_CHECKSTATUS
    }
}

fn twain_file_format_name(value: u16) -> &'static str {
    match value {
        0 => "TIFF",
        1 => "PICT",
        2 => "BMP",
        3 => "XBM",
        4 => "JPEG",
        5 => "FPX",
        6 => "multi-page TIFF",
        7 => "PNG",
        8 => "SPIFF",
        9 => "EXIF",
        10 => "PDF",
        11 => "JPEG 2000",
        13 => "JPX",
        14 => "DjVu",
        15 => "PDF/A",
        16 => "PDF/A-2",
        _ => "unknown",
    }
}

fn twain_compression_name(value: u16) -> &'static str {
    match value {
        0 => "none",
        1 => "PackBits",
        2 => "G3 1D",
        3 => "G3 1D EOL",
        4 => "G3 2D",
        5 => "G4",
        6 => "JPEG",
        7 => "LZW",
        8 => "JBIG",
        9 => "PNG",
        10 => "RLE4",
        11 => "RLE8",
        12 => "bitfields",
        13 => "ZIP",
        14 => "JPEG 2000",
        _ => "unknown",
    }
}

fn named_options(values: Option<Vec<String>>, name: fn(u16) -> &'static str) -> String {
    values.map_or("unavailable".to_owned(), |values| {
        values
            .iter()
            .map(|value| {
                value.parse::<u16>().map_or(value.clone(), |number| {
                    format!("{}({number})", name(number))
                })
            })
            .collect::<Vec<_>>()
            .join(", ")
    })
}

/// Lists what the driver offers for color in each transfer mode, within this probe session.
fn transfer_mode_report(
    entry: DsmEntry,
    app: &mut TwIdentity,
    source: &mut TwIdentity,
) -> Vec<String> {
    let color = twain_set(entry, app, source, ICAP_PIXELTYPE, TWTY_UINT16, 2);
    let mut lines = vec![format!("transfer test: color selected={color}")];
    for (mode, name) in [(2, "memory"), (1, "file"), (0, "native")] {
        let selected = twain_set(entry, app, source, ICAP_XFERMECH, TWTY_UINT16, mode);
        let formats = twain_get(entry, app, source, 0x110c, MSG_GET).map(|(_, values, _)| values);
        let compressions =
            twain_get(entry, app, source, ICAP_COMPRESSION, MSG_GET).map(|(_, values, _)| values);
        lines.push(format!(
            "transfer {name}: selected={selected}; file formats: {}; compression: {}",
            named_options(formats, twain_file_format_name),
            named_options(compressions, twain_compression_name)
        ));
    }
    lines
}

/// Applies a model's long-document mode and returns the mode the driver reports.
fn apply_long_document_mode(
    entry: DsmEntry,
    app: &mut TwIdentity,
    source: &mut TwIdentity,
    cap: u16,
    wanted_mm: u32,
) -> Option<String> {
    let (item_type, values, _) = twain_get(entry, app, source, cap, MSG_GET)?;
    let options = values
        .iter()
        .filter_map(|value| value.parse::<u32>().ok())
        .collect::<Vec<_>>();
    let mode = long_document_mode(&options, wanted_mm)?;
    if !twain_set(entry, app, source, cap, item_type, mode) {
        return None;
    }
    twain_get(entry, app, source, cap, MSG_GETCURRENT).map(|(_, _, current)| current)
}

fn device_profile(manufacturer: &str, product: &str) -> Option<&'static DeviceProfile> {
    DEVICE_PROFILES
        .iter()
        .find(|profile| (profile.matches)(manufacturer, product))
}
const PREVIEW_WIDTH: usize = 640;
const PREVIEW_MAX_ROWS: usize = 3000;

#[derive(Default)]
struct Pane {
    source_width: usize,
    step: usize,
    width: usize,
    stride: usize,
    rows: usize,
    keep_every: usize,
    seen: usize,
    pixels: Vec<u8>,
    label: String,
    shown_pixels: Vec<u8>,
    shown_width: usize,
    shown_rows: usize,
    shown_source_width: usize,
    shown_source_rows: usize,
}

impl Pane {
    fn begin(&mut self, source_width: usize, label: String) {
        self.source_width = source_width;
        self.step = source_width.div_ceil(PREVIEW_WIDTH).max(1);
        self.width = source_width / self.step;
        self.stride = (self.width * 3 + 3) & !3;
        self.rows = 0;
        self.keep_every = self.step;
        self.seen = 0;
        self.pixels.clear();
        self.label = label;
    }

    /// Replaces the visible image only when a new side is ready.
    fn publish(&mut self) {
        if self.rows > 0 {
            std::mem::swap(&mut self.pixels, &mut self.shown_pixels);
            (self.shown_width, self.shown_rows) = (self.width, self.rows);
            (self.shown_source_width, self.shown_source_rows) = (self.source_width, self.seen);
            self.pixels.clear();
            self.rows = 0;
        }
    }

    fn push(&mut self, row: &[u8], channels: usize) {
        let keep = self.seen.is_multiple_of(self.keep_every);
        self.seen += 1;
        if !keep || self.stride == 0 {
            return;
        }
        let start = self.pixels.len();
        self.pixels.resize(start + self.stride, 0);
        for x in 0..self.width {
            let source = x * self.step * channels;
            let target = start + x * 3;
            let (red, green, blue) = if channels == 3 {
                (row[source], row[source + 1], row[source + 2])
            } else {
                (row[source], row[source], row[source])
            };
            self.pixels[target] = blue;
            self.pixels[target + 1] = green;
            self.pixels[target + 2] = red;
        }
        self.rows += 1;
        if self.rows == PREVIEW_MAX_ROWS {
            for index in 1..self.rows / 2 {
                let (kept, moved) = self.pixels.split_at_mut(index * 2 * self.stride);
                kept[index * self.stride..(index + 1) * self.stride]
                    .copy_from_slice(&moved[..self.stride]);
            }
            self.rows /= 2;
            self.pixels.truncate(self.rows * self.stride);
            self.keep_every *= 2;
        }
    }
}

const UI_TOP: i32 = 56;
const UI_LOG_HEIGHT: i32 = 170;
const UI_STRIP_WIDTH: i32 = 200;
const UI_SHEET_HEIGHT: i32 = 150;
const THUMB_WIDTH: usize = 84;
// Warm pastel palette as Windows COLORREF values (0x00BBGGRR).
const UI_CREAM: u32 = 0x00e7_f3fb;
const UI_SELECTED: u32 = 0x00b8_d9f3;
const UI_TEXT: u32 = 0x0032_3b4a;
const UI_SAND: u32 = 0x00c3_d8e8;
const UI_SAGE: u32 = 0x00b0_d8bf;
const UI_HONEY: u32 = 0x009a_e0f6;
const UI_PEACH: u32 = 0x00a6_c9f8;
const UI_RED: u32 = 0x003c_3cd9;
const UI_WHITE: u32 = 0x00ff_ffff;
const UI_SCROLL: u32 = 0x008f_a4b9;
const UI_ZOOM_PANEL: u32 = 0x00cf_e4f3;
const ZOOM_PANEL_WIDTH: i32 = 372;
const MIN_TOOLBAR_RIGHT: i32 = 950;

#[derive(Default)]
struct ViewImage {
    width: usize,
    rows: usize,
    pixels: Vec<u8>,
}

struct SessionImage {
    sheet: u32,
    file: String,
    source_width: usize,
    source_rows: usize,
    side: Side,
    width: usize,
    rows: usize,
    offset: u64,
    bytes: usize,
    thumb: ViewImage,
}

/// Keeps small thumbnails in memory and larger session previews in a temporary file.
#[derive(Default)]
struct Session {
    images: Vec<SessionImage>,
    cache: Option<File>,
    cache_path: Option<PathBuf>,
    cache_length: u64,
    selected: Option<u32>,
    pinned: bool,
    scroll: i32,
    view: [ViewImage; 2],
}

impl Session {
    fn open_cache(&mut self, path: PathBuf) -> io::Result<()> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        self.cache = Some(file);
        self.cache_path = Some(path);
        Ok(())
    }

    fn add(&mut self, sheet: u32, side: Side, file: &str, pane: &Pane) {
        if pane.shown_rows == 0 || pane.shown_width == 0 {
            return;
        }
        let stride = (pane.shown_width * 3 + 3) & !3;
        let bytes = pane.shown_rows * stride;
        let offset = self.cache_length;
        let stored = self.cache.as_mut().is_some_and(|cache| {
            cache.seek(SeekFrom::Start(offset)).is_ok()
                && cache.write_all(&pane.shown_pixels[..bytes]).is_ok()
        });
        if stored {
            self.cache_length += bytes as u64;
        } else if self.cache.take().is_some() {
            trace(
                "session",
                "preview cache failed; showing thumbnails for older sheets",
            );
        }
        self.images.push(SessionImage {
            sheet,
            file: file.to_owned(),
            source_width: pane.shown_source_width,
            source_rows: pane.shown_source_rows,
            side,
            width: pane.shown_width,
            rows: pane.shown_rows,
            offset,
            bytes: if stored { bytes } else { 0 },
            thumb: thumbnail(pane, stride),
        });
    }

    fn sheets(&self) -> Vec<u32> {
        let mut sheets = Vec::new();
        for image in &self.images {
            if sheets.last() != Some(&image.sheet) && !sheets.contains(&image.sheet) {
                sheets.push(image.sheet);
            }
        }
        sheets
    }

    fn select(&mut self, sheet: u32) {
        self.selected = Some(sheet);
        self.pinned = true;
        for (index, side) in [Side::Front, Side::Back].into_iter().enumerate() {
            let found = self
                .images
                .iter()
                .rposition(|image| image.sheet == sheet && image.side == side);
            self.view[index] = found.map_or_else(ViewImage::default, |at| self.load(at));
        }
    }

    fn load(&mut self, at: usize) -> ViewImage {
        let (offset, bytes, width, rows) = {
            let image = &self.images[at];
            (image.offset, image.bytes, image.width, image.rows)
        };
        if bytes > 0
            && let Some(cache) = self.cache.as_mut()
        {
            let mut pixels = vec![0; bytes];
            if cache.seek(SeekFrom::Start(offset)).is_ok() && cache.read_exact(&mut pixels).is_ok()
            {
                return ViewImage {
                    width,
                    rows,
                    pixels,
                };
            }
        }
        let thumb = &self.images[at].thumb;
        ViewImage {
            width: thumb.width,
            rows: thumb.rows,
            pixels: thumb.pixels.clone(),
        }
    }

    /// Moves every saved side of a sheet to `.deleted` and removes it from the session.
    fn delete_sheet(&mut self, sheet: u32, directory: &Path) -> io::Result<Vec<String>> {
        let files = self
            .images
            .iter()
            .filter(|image| image.sheet == sheet)
            .map(|image| image.file.clone())
            .collect::<Vec<_>>();
        let moved = move_to_deleted(directory, &files)?;
        self.images.retain(|image| image.sheet != sheet);
        if self.selected == Some(sheet) {
            self.follow_live();
        }
        Ok(moved)
    }

    /// Pins a sheet, or returns to following the newest sheet on a second click.
    fn toggle_sheet(&mut self, sheet: u32) {
        if self.selected == Some(sheet) {
            self.follow_live();
        } else {
            self.select(sheet);
        }
    }

    fn follow_live(&mut self) {
        self.selected = None;
        self.pinned = false;
        self.view = Default::default();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cache = None;
        if let Some(path) = self.cache_path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Moves files into `.deleted` without overwriting an earlier deleted file.
fn move_to_deleted(directory: &Path, files: &[String]) -> io::Result<Vec<String>> {
    let deleted = directory.join(".deleted");
    fs::create_dir_all(&deleted)?;
    let mut moved = Vec::new();
    for file in files.iter().filter(|file| !file.is_empty()) {
        let source = directory.join(file);
        if !source.exists() {
            continue;
        }
        let mut target = deleted.join(file);
        let mut copy = 1;
        loop {
            match fs::hard_link(&source, &target) {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    target = deleted.join(format!("{file}.{copy}"));
                    copy += 1;
                }
                Err(error) => return Err(error),
            }
        }
        fs::remove_file(&source)?;
        moved.push(file.clone());
    }
    Ok(moved)
}

fn thumbnail(pane: &Pane, stride: usize) -> ViewImage {
    let step = pane.shown_width.div_ceil(THUMB_WIDTH).max(1);
    let width = pane.shown_width / step;
    let rows = (pane.shown_rows / step).max(1);
    let thumb_stride = (width * 3 + 3) & !3;
    let mut pixels = vec![0; thumb_stride * rows];
    for y in 0..rows {
        let source = (y * step).min(pane.shown_rows - 1) * stride;
        let target = y * thumb_stride;
        for x in 0..width {
            pixels[target + x * 3..target + x * 3 + 3].copy_from_slice(
                &pane.shown_pixels[source + x * step * 3..source + x * step * 3 + 3],
            );
        }
    }
    ViewImage {
        width,
        rows,
        pixels,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum ZoomMode {
    #[default]
    Full,
    Width,
    Height,
    Normal,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ZoomAction {
    Full,
    Width,
    Height,
    Normal,
    Out,
    In,
    Deskew,
}

const ZOOM_BUTTONS: [(&str, ZoomAction); 7] = [
    ("Full page", ZoomAction::Full),
    ("Fit width", ZoomAction::Width),
    ("Fit height", ZoomAction::Height),
    ("Real scan pixels 100%", ZoomAction::Normal),
    ("Zoom out", ZoomAction::Out),
    ("Zoom in", ZoomAction::In),
    ("Deskew view (original unchanged)", ZoomAction::Deskew),
];
const ZOOM_STEP: f64 = 1.25;
const ZOOM_MIN: f64 = 0.01;
const MAX_FULL_IMAGE_BYTES: usize = 512 * 1024 * 1024;
const ZOOM_MAX: f64 = 4.0;

/// The shared front/back view: fit mode, current scale, and pan offset.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewZoom {
    mode: ZoomMode,
    fit: ZoomMode,
    scale: f64,
    pan_x: i32,
    pan_y: i32,
}

impl Default for ViewZoom {
    fn default() -> Self {
        Self {
            mode: ZoomMode::Full,
            fit: ZoomMode::Width,
            scale: 1.0,
            pan_x: 0,
            pan_y: 0,
        }
    }
}

impl ViewZoom {
    fn scale_for(&self, width: usize, rows: usize, pane_width: i32, pane_height: i32) -> f64 {
        let fit_width = f64::from(pane_width) / width.max(1) as f64;
        let fit_height = f64::from(pane_height) / rows.max(1) as f64;
        let scale = match self.mode {
            ZoomMode::Full => fit_width.min(fit_height),
            ZoomMode::Width => fit_width,
            ZoomMode::Height => fit_height,
            ZoomMode::Normal => 1.0,
            ZoomMode::Custom => self.scale,
        };
        scale.clamp(ZOOM_MIN, ZOOM_MAX)
    }

    fn apply(&mut self, action: ZoomAction) {
        let fit = |zoom: &mut Self, mode| {
            zoom.mode = mode;
            if mode != ZoomMode::Normal {
                zoom.fit = mode;
            }
            zoom.pan_x = 0;
            zoom.pan_y = 0;
        };
        match action {
            ZoomAction::Full => fit(self, ZoomMode::Full),
            ZoomAction::Width => fit(self, ZoomMode::Width),
            ZoomAction::Height => fit(self, ZoomMode::Height),
            ZoomAction::Normal => fit(self, ZoomMode::Normal),
            ZoomAction::Out => {
                self.scale = (self.scale / ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
                self.mode = ZoomMode::Custom;
            }
            ZoomAction::In => {
                self.scale = (self.scale * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
                self.mode = ZoomMode::Custom;
            }
            ZoomAction::Deskew => {}
        }
    }

    /// Double-click inspection: jump to real pixels here, or return to the fit view.
    fn toggle_inspect(&mut self, anchor_x: i32, anchor_y: i32) {
        if self.mode == ZoomMode::Normal {
            self.mode = self.fit;
            self.pan_x = 0;
            self.pan_y = 0;
            return;
        }
        let old = self.scale.max(ZOOM_MIN);
        let content_x = (f64::from(self.pan_x) + f64::from(anchor_x)) / old;
        let content_y = (f64::from(self.pan_y) + f64::from(anchor_y)) / old;
        self.mode = ZoomMode::Normal;
        self.scale = 1.0;
        self.pan_x = ((content_x - f64::from(anchor_x)).round() as i32).max(0);
        self.pan_y = ((content_y - f64::from(anchor_y)).round() as i32).max(0);
    }

    /// Zooms while keeping the image point under the anchor in place.
    fn zoom_at(&mut self, action: ZoomAction, anchor_x: i32, anchor_y: i32) {
        let old = self.scale.max(ZOOM_MIN);
        let content_x = (f64::from(self.pan_x) + f64::from(anchor_x)) / old;
        let content_y = (f64::from(self.pan_y) + f64::from(anchor_y)) / old;
        self.apply(action);
        self.pan_x = ((content_x * self.scale - f64::from(anchor_x)).round() as i32).max(0);
        self.pan_y = ((content_y * self.scale - f64::from(anchor_y)).round() as i32).max(0);
    }

    fn is_active(&self, action: ZoomAction) -> bool {
        matches!(
            (self.mode, action),
            (ZoomMode::Full, ZoomAction::Full)
                | (ZoomMode::Width, ZoomAction::Width)
                | (ZoomMode::Height, ZoomAction::Height)
                | (ZoomMode::Normal, ZoomAction::Normal)
        )
    }
}

/// Returns the scroll indicator start and length inside a viewport.
fn scroll_thumb(viewport: i32, content: i32, offset: i32) -> (i32, i32) {
    let viewport = viewport.max(1);
    let content = content.max(viewport);
    let length = (i64::from(viewport) * i64::from(viewport) / i64::from(content)) as i32;
    let length = length.clamp(18.min(viewport), viewport);
    let range = content - viewport;
    let start = if range == 0 {
        0
    } else {
        (i64::from(offset.clamp(0, range)) * i64::from(viewport - length) / i64::from(range)) as i32
    };
    (start, length)
}

fn zoom_percent(scale: f64) -> String {
    format!("{:.0}%", scale * 100.0)
}

/// The labeled Zoom group is aligned to the right edge of the window.
fn zoom_panel_rect(toolbar_right: i32) -> WinRect {
    let right = toolbar_right.max(MIN_TOOLBAR_RIGHT);
    WinRect {
        left: right - ZOOM_PANEL_WIDTH,
        top: 6,
        right,
        bottom: 48,
    }
}

/// Zoom buttons sit inside the group; Deskew is a separate button before it.
fn zoom_button_rect(index: usize, toolbar_right: i32) -> WinRect {
    let panel = zoom_panel_rect(toolbar_right);
    let left = if index < 6 {
        panel.left + 54 + index as i32 * 42
    } else {
        panel.left - 50
    };
    WinRect {
        left,
        top: 10,
        right: left + 38,
        bottom: 44,
    }
}

fn zoom_percent_x(toolbar_right: i32) -> i32 {
    zoom_panel_rect(toolbar_right).left + 314
}
struct DeskewView {
    key: (Option<u32>, u32, usize, usize),
    angle: Option<f64>,
    image: Option<ViewImage>,
}

/// Estimates the rotation that makes text lines horizontal, in degrees.
fn detect_skew(pixels: &[u8], width: usize, rows: usize) -> Option<f64> {
    if width == 0 || rows == 0 {
        return None;
    }
    let stride = (width * 3 + 3) & !3;
    let step = (width.max(rows) / 500).max(1);
    let mut histogram = [0u32; 256];
    let gray = |x: usize, y: usize| {
        let at = y * stride + x * 3;
        ((u32::from(pixels[at]) + u32::from(pixels[at + 1]) + u32::from(pixels[at + 2])) / 3) as u8
    };
    let mut samples = 0u32;
    for y in (0..rows).step_by(step) {
        for x in (0..width).step_by(step) {
            histogram[usize::from(gray(x, y))] += 1;
            samples += 1;
        }
    }
    let mut seen = 0;
    let mut light = 255u8;
    for (value, count) in histogram.iter().enumerate() {
        seen += count;
        if seen * 10 >= samples * 9 {
            light = value as u8;
            break;
        }
    }
    let threshold = light.saturating_sub(60);
    let (center_x, center_y) = (width as f64 / 2.0, rows as f64 / 2.0);
    let mut points = Vec::new();
    for y in (0..rows).step_by(step) {
        for x in (0..width).step_by(step) {
            if gray(x, y) < threshold {
                points.push((
                    (x as f64 - center_x) / step as f64,
                    (y as f64 - center_y) / step as f64,
                ));
            }
        }
    }
    if points.len() < 80 || points.len() as u32 * 10 > samples * 6 {
        return None;
    }
    let diagonal =
        (((width * width + rows * rows) as f64).sqrt() / step as f64).ceil() as usize + 2;
    let mut bins = vec![0i64; diagonal * 2 + 2];
    let mut score = |degrees: f64| {
        bins.fill(0);
        let (sin, cos) = degrees.to_radians().sin_cos();
        for (x, y) in &points {
            let line = (sin * x + cos * y + diagonal as f64).round() as usize;
            bins[line] += 1;
        }
        bins.windows(2)
            .map(|pair| (pair[1] - pair[0]) * (pair[1] - pair[0]))
            .sum::<i64>() as f64
    };
    let (mut best_angle, mut best_score, mut total) = (0.0, f64::MIN, 0.0);
    for degrees in -45..=45 {
        let value = score(f64::from(degrees));
        total += value;
        if value > best_score {
            (best_angle, best_score) = (f64::from(degrees), value);
        }
    }
    if total <= 0.0 || best_score / (total / 91.0) < 1.2 {
        return None;
    }
    let coarse = best_angle;
    for tenth in -10..=10 {
        let degrees = coarse + f64::from(tenth) / 10.0;
        let value = score(degrees);
        if value > best_score {
            (best_angle, best_score) = (degrees, value);
        }
    }
    Some(best_angle)
}

/// Rotates a preview into a new bitmap on a white background.
fn rotate_bgr(pixels: &[u8], width: usize, rows: usize, degrees: f64) -> ViewImage {
    let stride = (width * 3 + 3) & !3;
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (w, h) = (width as f64, rows as f64);
    let new_width = (w * cos.abs() + h * sin.abs()).ceil().max(1.0) as usize;
    let new_rows = (w * sin.abs() + h * cos.abs()).ceil().max(1.0) as usize;
    let new_stride = (new_width * 3 + 3) & !3;
    let mut output = vec![255u8; new_stride * new_rows];
    let (source_x, source_y) = ((w - 1.0) / 2.0, (h - 1.0) / 2.0);
    let (target_x, target_y) = (
        (new_width as f64 - 1.0) / 2.0,
        (new_rows as f64 - 1.0) / 2.0,
    );
    for y in 0..new_rows {
        let dy = y as f64 - target_y;
        for x in 0..new_width {
            let dx = x as f64 - target_x;
            let sx = cos * dx + sin * dy + source_x;
            let sy = -sin * dx + cos * dy + source_y;
            if sx < 0.0 || sy < 0.0 || sx >= w - 1.0 || sy >= h - 1.0 {
                continue;
            }
            let (left, top) = (sx as usize, sy as usize);
            let (fx, fy) = (sx - left as f64, sy - top as f64);
            for channel in 0..3 {
                let sample =
                    |px: usize, py: usize| f64::from(pixels[py * stride + px * 3 + channel]);
                let upper = sample(left, top) * (1.0 - fx) + sample(left + 1, top) * fx;
                let lower = sample(left, top + 1) * (1.0 - fx) + sample(left + 1, top + 1) * fx;
                output[y * new_stride + x * 3 + channel] =
                    (upper * (1.0 - fy) + lower * fy).round() as u8;
            }
        }
    }
    ViewImage {
        width: new_width,
        rows: new_rows,
        pixels: output,
    }
}

fn refresh_deskew(ui: &mut UiState) {
    if !ui.deskew {
        return;
    }
    let (selected, saved) = (ui.session.selected, ui.saved);
    for index in 0..2 {
        let computed = {
            let (pixels, width, rows): (&[u8], usize, usize) = if selected.is_some() {
                let image = &ui.session.view[index];
                (&image.pixels, image.width, image.rows)
            } else {
                let pane = &ui.panes[index];
                (&pane.shown_pixels, pane.shown_width, pane.shown_rows)
            };
            let key = (selected, saved, width, rows);
            if width == 0 || rows == 0 {
                Some(None)
            } else if ui.deskew_views[index]
                .as_ref()
                .is_some_and(|view| view.key == key)
            {
                None
            } else {
                let angle = detect_skew(pixels, width, rows);
                let image = angle
                    .filter(|angle| angle.abs() >= 0.15)
                    .map(|angle| rotate_bgr(pixels, width, rows, angle));
                Some(Some(DeskewView { key, angle, image }))
            }
        };
        if let Some(view) = computed {
            ui.deskew_views[index] = view;
        }
    }
}

struct FullView {
    file: String,
    image: ViewImage,
}

struct DisplayImage<'a> {
    pixels: &'a [u8],
    width: usize,
    rows: usize,
    source_width: usize,
    source_rows: usize,
}

fn current_session_image(ui: &UiState, index: usize) -> Option<&SessionImage> {
    let side = if index == 0 { Side::Front } else { Side::Back };
    let sheet = ui
        .session
        .selected
        .or_else(|| ui.session.sheets().last().copied())?;
    ui.session
        .images
        .iter()
        .rev()
        .find(|image| image.sheet == sheet && image.side == side)
}

/// Chooses the best available bitmap while keeping dimensions in real scan pixels.
fn displayed_image(ui: &UiState, index: usize) -> Option<DisplayImage<'_>> {
    let session_image = current_session_image(ui, index);
    let (pixels, width, rows, source_width, source_rows): (&[u8], usize, usize, usize, usize) =
        if ui.session.selected.is_some() {
            let view = &ui.session.view[index];
            let (source_width, source_rows) = session_image
                .map_or((view.width, view.rows), |image| {
                    (image.source_width, image.source_rows)
                });
            (
                &view.pixels,
                view.width,
                view.rows,
                source_width,
                source_rows,
            )
        } else {
            let pane = &ui.panes[index];
            (
                &pane.shown_pixels,
                pane.shown_width,
                pane.shown_rows,
                pane.shown_source_width,
                pane.shown_source_rows,
            )
        };
    if width == 0 || rows == 0 {
        return None;
    }
    if ui.deskew
        && let Some(view) = ui.deskew_views[index]
            .as_ref()
            .and_then(|view| view.image.as_ref())
    {
        let ratio_x = source_width as f64 / width as f64;
        let ratio_y = source_rows as f64 / rows as f64;
        return Some(DisplayImage {
            pixels: &view.pixels,
            width: view.width,
            rows: view.rows,
            source_width: (view.width as f64 * ratio_x).round() as usize,
            source_rows: (view.rows as f64 * ratio_y).round() as usize,
        });
    }
    if !ui.deskew
        && let (Some(full), Some(image)) = (&ui.full[index], session_image)
        && full.file == image.file
        && full.image.width == source_width
        && full.image.rows == source_rows
    {
        return Some(DisplayImage {
            pixels: &full.image.pixels,
            width: full.image.width,
            rows: full.image.rows,
            source_width,
            source_rows,
        });
    }
    Some(DisplayImage {
        pixels,
        width,
        rows,
        source_width,
        source_rows,
    })
}

/// Loads real pixels in the background when the view is larger than the preview.
fn request_full_images(shared: &Arc<ScanShared>) {
    let mut jobs = Vec::new();
    {
        let mut ui = shared.ui.lock().expect("ui lock");
        if ui.deskew {
            return;
        }
        let area = ui.pane_area;
        let pane_width = ((area.right - area.left - 10) / 2).max(1);
        let pane_height = (area.bottom - area.top).max(1);
        for index in 0..2 {
            let Some(image) = displayed_image(&ui, index) else {
                continue;
            };
            if image.width >= image.source_width {
                continue;
            }
            let scale = ui.zoom.scale_for(
                image.source_width,
                image.source_rows,
                pane_width,
                pane_height,
            );
            if image.source_width as f64 * scale <= image.width as f64 * 1.05 {
                continue;
            }
            let Some(file) = current_session_image(&ui, index).map(|image| image.file.clone())
            else {
                continue;
            };
            if ui.full_pending[index].as_deref() == Some(file.as_str()) {
                continue;
            }
            ui.full_pending[index] = Some(file.clone());
            jobs.push((index, ui.output_dir.join(&file), file));
        }
    }
    for (index, path, file) in jobs {
        let shared = Arc::clone(shared);
        thread::spawn(move || match decode_png_bgr(&path) {
            Ok(image) => {
                let mut ui = shared.ui.lock().expect("ui lock");
                if ui.full_pending[index].as_deref() == Some(file.as_str()) {
                    ui.full[index] = Some(FullView { file, image });
                    ui.version += 1;
                }
            }
            Err(error) => trace("warning", format!("cannot load full image {file}: {error}")),
        });
    }
}

struct InflateBits<'a> {
    data: &'a [u8],
    position: usize,
    bits: u64,
    count: u32,
}

impl InflateBits<'_> {
    fn fill(&mut self, wanted: u32) {
        while self.count < wanted {
            let byte = self.data.get(self.position).copied().unwrap_or(0);
            self.position += 1;
            self.bits |= u64::from(byte) << self.count;
            self.count += 8;
        }
    }

    fn take(&mut self, count: u32) -> u32 {
        if count == 0 {
            return 0;
        }
        self.fill(count);
        let value = (self.bits & ((1u64 << count) - 1)) as u32;
        self.bits >>= count;
        self.count -= count;
        value
    }

    fn overrun(&self) -> bool {
        self.position > self.data.len() + 8
    }
}

struct InflateTable {
    entries: Vec<u16>,
    bits: u32,
}

impl InflateTable {
    fn new(lengths: &[u8]) -> io::Result<Self> {
        let bits = u32::from(lengths.iter().copied().max().unwrap_or(0)).max(1);
        let size = 1usize << bits;
        let mut entries = vec![0u16; size];
        let codes = canonical_codes(lengths);
        for (symbol, (&length, &code)) in lengths.iter().zip(&codes).enumerate() {
            if length == 0 {
                continue;
            }
            let length = u32::from(length);
            if code >= 1 << length {
                return Err(invalid_png("over-subscribed Huffman code"));
            }
            let reversed = (code.reverse_bits() >> (32 - length)) as usize;
            let entry = ((symbol as u16) << 4) | length as u16;
            for slot in (reversed..size).step_by(1 << length) {
                entries[slot] = entry;
            }
        }
        Ok(Self { entries, bits })
    }

    fn symbol(&self, input: &mut InflateBits<'_>) -> io::Result<usize> {
        input.fill(self.bits);
        let entry = self.entries[(input.bits & ((1u64 << self.bits) - 1)) as usize];
        if entry == 0 {
            return Err(invalid_png("invalid Huffman code"));
        }
        let length = u32::from(entry & 0xf);
        input.bits >>= length;
        input.count -= length;
        Ok(usize::from(entry >> 4))
    }
}

fn invalid_png(message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid PNG: {message}"),
    )
}

/// Decompresses a raw DEFLATE stream with a known output size.
fn inflate_raw(data: &[u8], expected: usize) -> io::Result<Vec<u8>> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut input = InflateBits {
        data,
        position: 0,
        bits: 0,
        count: 0,
    };
    let mut output = Vec::with_capacity(expected);
    loop {
        let last = input.take(1) == 1;
        let kind = input.take(2);
        if kind == 0 {
            let drop_bits = input.count % 8;
            input.take(drop_bits);
            let length = input.take(16) as usize;
            let inverse = input.take(16) as usize;
            if length ^ inverse != 0xffff || output.len() + length > expected {
                return Err(invalid_png("bad stored block"));
            }
            for _ in 0..length {
                output.push(input.take(8) as u8);
            }
        } else {
            let (literals, distances) = if kind == 1 {
                let mut literal_lengths = [8u8; 288];
                literal_lengths[144..256].fill(9);
                literal_lengths[256..280].fill(7);
                (
                    InflateTable::new(&literal_lengths)?,
                    InflateTable::new(&[5u8; 30])?,
                )
            } else if kind == 2 {
                let literal_count = input.take(5) as usize + 257;
                let distance_count = input.take(5) as usize + 1;
                let code_count = input.take(4) as usize + 4;
                let mut code_lengths = [0u8; 19];
                for symbol in &ORDER[..code_count] {
                    code_lengths[*symbol] = input.take(3) as u8;
                }
                let code_table = InflateTable::new(&code_lengths)?;
                let total = literal_count + distance_count;
                let mut lengths = Vec::with_capacity(total);
                while lengths.len() < total {
                    match code_table.symbol(&mut input)? {
                        16 => {
                            let previous = *lengths
                                .last()
                                .ok_or_else(|| invalid_png("repeat without a length"))?;
                            let count = 3 + input.take(2) as usize;
                            lengths.extend(std::iter::repeat_n(previous, count));
                        }
                        17 => {
                            let count = 3 + input.take(3) as usize;
                            lengths.extend(std::iter::repeat_n(0, count));
                        }
                        18 => {
                            let count = 11 + input.take(7) as usize;
                            lengths.extend(std::iter::repeat_n(0, count));
                        }
                        value => lengths.push(value as u8),
                    }
                }
                if lengths.len() != total {
                    return Err(invalid_png("bad code lengths"));
                }
                (
                    InflateTable::new(&lengths[..literal_count])?,
                    InflateTable::new(&lengths[literal_count..])?,
                )
            } else {
                return Err(invalid_png("reserved block type"));
            };
            loop {
                if input.overrun() {
                    return Err(invalid_png("truncated stream"));
                }
                match literals.symbol(&mut input)? {
                    symbol @ 0..=255 => {
                        if output.len() == expected {
                            return Err(invalid_png("too much image data"));
                        }
                        output.push(symbol as u8);
                    }
                    256 => break,
                    symbol => {
                        let index = symbol - 257;
                        if index >= LEN_BASE.len() {
                            return Err(invalid_png("bad length code"));
                        }
                        let length = usize::from(LEN_BASE[index])
                            + input.take(u32::from(LEN_EXTRA[index])) as usize;
                        let index = distances.symbol(&mut input)?;
                        if index >= DIST_BASE.len() {
                            return Err(invalid_png("bad distance code"));
                        }
                        let distance = usize::from(DIST_BASE[index])
                            + input.take(u32::from(DIST_EXTRA[index])) as usize;
                        if distance > output.len() || output.len() + length > expected {
                            return Err(invalid_png("bad match"));
                        }
                        let start = output.len() - distance;
                        if distance >= length {
                            output.extend_from_within(start..start + length);
                        } else {
                            for offset in 0..length {
                                output.push(output[start + offset]);
                            }
                        }
                    }
                }
            }
        }
        if last {
            break;
        }
    }
    if output.len() != expected {
        return Err(invalid_png("wrong image size"));
    }
    Ok(output)
}

/// Decodes an 8-bit RGB or grayscale PNG into a Windows BGR bitmap.
fn decode_png_bgr(path: &Path) -> io::Result<ViewImage> {
    let bytes = fs::read(path)?;
    if bytes.len() < 33 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(invalid_png("missing signature"));
    }
    let (mut at, mut idat) = (8, Vec::with_capacity(bytes.len()));
    let mut header = None;
    while at + 12 <= bytes.len() {
        let length =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let end = at
            .checked_add(12)
            .and_then(|value| value.checked_add(length))
            .filter(|value| *value <= bytes.len())
            .ok_or_else(|| invalid_png("truncated chunk"))?;
        let kind = &bytes[at + 4..at + 8];
        let data = &bytes[at + 8..at + 8 + length];
        match kind {
            b"IHDR" if length == 13 => header = Some(data.to_vec()),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        at = end;
    }
    let header = header.ok_or_else(|| invalid_png("missing header"))?;
    let width = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let rows = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let channels = match (header[8], header[9], header[12]) {
        (8, 2, 0) => 3,
        (8, 0, 0) => 1,
        _ => return Err(invalid_png("unsupported format")),
    };
    let target_stride = (width * 3 + 3) & !3;
    if width == 0
        || rows == 0
        || target_stride
            .checked_mul(rows)
            .is_none_or(|bytes| bytes > MAX_FULL_IMAGE_BYTES)
    {
        return Err(invalid_png("image is too large to open in full resolution"));
    }
    if idat.len() < 6 {
        return Err(invalid_png("missing image data"));
    }
    let stride = width * channels;
    let raw = inflate_raw(&idat[2..idat.len() - 4], (stride + 1) * rows)?;
    drop(idat);
    let mut output = vec![0u8; target_stride * rows];
    let mut previous = vec![0u8; stride];
    let mut current = vec![0u8; stride];
    for y in 0..rows {
        let line = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        let filter = line[0];
        for x in 0..stride {
            let left = if x >= channels {
                current[x - channels]
            } else {
                0
            };
            let up = previous[x];
            let up_left = if x >= channels {
                previous[x - channels]
            } else {
                0
            };
            let prediction = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => paeth(left, up, up_left),
                _ => return Err(invalid_png("bad filter")),
            };
            current[x] = line[x + 1].wrapping_add(prediction);
        }
        let target = &mut output[y * target_stride..(y + 1) * target_stride];
        for x in 0..width {
            let (red, green, blue) = if channels == 3 {
                (current[x * 3], current[x * 3 + 1], current[x * 3 + 2])
            } else {
                (current[x], current[x], current[x])
            };
            target[x * 3] = blue;
            target[x * 3 + 1] = green;
            target[x * 3 + 2] = red;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    Ok(ViewImage {
        width,
        rows,
        pixels: output,
    })
}

fn view_state_path(env: EnvLookup<'_>) -> Option<PathBuf> {
    xdg_dir(env, "XDG_STATE_HOME", ".local/state").map(|dir| dir.join("view.txt"))
}

fn parse_fit_mode(text: &str) -> ZoomMode {
    match text.trim() {
        "fit=full" => ZoomMode::Full,
        "fit=height" => ZoomMode::Height,
        _ => ZoomMode::Width,
    }
}

fn fit_mode_text(mode: ZoomMode) -> &'static str {
    match mode {
        ZoomMode::Full => "fit=full\n",
        ZoomMode::Height => "fit=height\n",
        _ => "fit=width\n",
    }
}

fn load_fit_mode() -> ZoomMode {
    view_state_path(&real_env)
        .and_then(|path| fs::read_to_string(path).ok())
        .map_or(ZoomMode::Width, |text| parse_fit_mode(&text))
}

fn save_fit_mode(mode: ZoomMode) {
    if let Some(path) = view_state_path(&real_env) {
        let saved = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, fit_mode_text(mode)));
        if let Err(error) = saved {
            trace("warning", format!("cannot remember zoom mode: {error}"));
        }
    }
}

#[derive(Default)]
struct UiState {
    status: String,
    detail: String,
    saved: u32,
    version: u64,
    panes: [Pane; 2],
    session: Session,
    driver_pending: String,
    driver_tags: String,
    processing: Option<u32>,
    phase: ScanPhase,
    output_dir: PathBuf,
    zoom: ViewZoom,
    drag: Option<(i32, i32, i32, i32)>,
    pane_area: WinRect,
    deskew: bool,
    deskew_views: [Option<DeskewView>; 2],
    hover_label: Option<&'static str>,
    full: [Option<FullView>; 2],
    full_pending: [Option<String>; 2],
    toolbar_right: i32,
    last_progress: Option<std::time::Instant>,
    close_notice: Option<String>,
    close_set_pause: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ScanPhase {
    #[default]
    Starting,
    WaitingForLock,
    WaitingForPaper,
    Scanning,
    Scanned,
    Attention,
    Jam,
    NotReady,
    Paused,
    Failed,
}

/// What a device-online change means while a paper fault is waiting to be fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FaultSignal {
    Unchanged,
    Opened,
    Closed,
}

fn fault_signal(was_online: bool, online: bool) -> FaultSignal {
    match (was_online, online) {
        (true, false) => FaultSignal::Opened,
        (false, true) => FaultSignal::Closed,
        _ => FaultSignal::Unchanged,
    }
}

/// A driver also hides the feeder state after a jam, so that alone cannot mean "cover open".
fn phase_when_feeder_unavailable(paper_fault_active: bool) -> Option<ScanPhase> {
    (!paper_fault_active).then_some(ScanPhase::NotReady)
}

impl ScanPhase {
    /// Returns the banner label and its Windows COLORREF (0x00BBGGRR).
    fn banner(self) -> (&'static str, u32) {
        match self {
            Self::Starting => ("STARTING", UI_SAGE),
            Self::WaitingForLock => ("LOCK WAIT", UI_RED),
            Self::WaitingForPaper => ("WAITING", UI_HONEY),
            Self::Scanning => ("SCANNING", UI_SAGE),
            Self::Scanned => ("SCANNED", UI_SAGE),
            Self::Attention => ("ATTENTION", UI_RED),
            Self::Jam => ("JAM", UI_RED),
            Self::NotReady => ("NOT READY", UI_RED),
            Self::Paused => ("PAUSED", UI_HONEY),
            Self::Failed => ("FAILED", UI_RED),
        }
    }

    fn text_color(self) -> u32 {
        if self.banner().1 == UI_RED {
            UI_WHITE
        } else {
            UI_TEXT
        }
    }
}

#[derive(Default)]
struct ScanShared {
    ui: Mutex<UiState>,
    stop: AtomicBool,
    continue_requested: AtomicBool,
    attention_cleared: AtomicBool,
    paper_removed: AtomicBool,
    paused: AtomicBool,
}

impl ScanShared {
    fn show(&self, status: impl Into<String>, detail: impl Into<String>) {
        let (status, detail) = (status.into(), detail.into());
        trace("status", format!("{status}: {detail}"));
        self.update(status, detail);
    }

    fn update(&self, status: String, detail: String) {
        let mut ui = self.ui.lock().expect("ui lock");
        ui.status = status;
        ui.detail = detail;
        ui.version += 1;
    }

    /// Shows a repeated wait message without repeating it in the log.
    fn show_once(&self, status: impl Into<String>, detail: impl Into<String>) {
        let (status, detail) = (status.into(), detail.into());
        let changed = {
            let ui = self.ui.lock().expect("ui lock");
            ui.status != status || ui.detail != detail
        };
        if changed {
            self.show(status, detail);
        }
    }

    fn set_phase(&self, phase: ScanPhase) {
        let mut ui = self.ui.lock().expect("ui lock");
        if ui.phase != phase {
            ui.phase = phase;
            ui.version += 1;
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();
static LOG_PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOG_CONTROL: AtomicIsize = AtomicIsize::new(0);

fn trace_line(stamp: &str, area: &str, message: &str) -> String {
    let time = if stamp.len() == 17 {
        format!(
            "{}:{}:{}.{}",
            &stamp[8..10],
            &stamp[10..12],
            &stamp[12..14],
            &stamp[14..17]
        )
    } else {
        stamp.to_owned()
    };
    format!("{time} [{area}] {message}")
}

/// Explains what the app detected or decided, on stderr and in ownscanner.log.
fn trace(area: &str, message: impl AsRef<str>) {
    let line = trace_line(&local_stamp(), area, message.as_ref());
    eprintln!("{line}");
    if let Some(file) = LOG_FILE.get() {
        let mut file = file.lock().expect("log lock");
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
        LOG_PENDING.lock().expect("log view lock").push(line);
    }
}

fn open_log(directory: &Path, prefix: &str, stamp: &str) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let path = directory.join(format!("{prefix}_{stamp}.log"));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let _ = LOG_FILE.set(Mutex::new(file));
    trace(
        "app",
        format!(
            "OwnScanner {VERSION} started; {}; log {}",
            build_identity(),
            path.display()
        ),
    );
    Ok(())
}

/// Names the exact executable, because local builds all share the same version number.
fn build_identity() -> String {
    let Ok(exe) = env::current_exe() else {
        return "build unknown".to_owned();
    };
    let build = sha256_file(&exe).map_or_else(
        |_| "unknown".to_owned(),
        |(hash, _)| hash.chars().take(12).collect(),
    );
    format!("build {build}; exe {}", exe.display())
}

fn running_holder(holders: &[(&str, &'static str)]) -> Option<&'static str> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = Command::new("tasklist")
        .args(["/fo", "csv", "/nh"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    holders
        .iter()
        .find(|(process, _)| text.contains(&format!("\"{}\"", process.to_ascii_lowercase())))
        .map(|(_, name)| *name)
}

#[repr(C, packed(2))]
struct TwUserInterface {
    show_ui: u16,
    modal_ui: u16,
    parent: Handle,
}

#[repr(C, packed(2))]
struct TwEvent {
    event: *mut WinMsg,
    message: u16,
}

#[repr(C, packed(2))]
#[derive(Default)]
struct TwPendingXfers {
    count: u16,
    eoj: u32,
}

#[repr(C, packed(2))]
#[derive(Default)]
struct TwSetupMemXfer {
    minimum: u32,
    preferred: u32,
    maximum: u32,
}

#[repr(C, packed(2))]
#[derive(Default)]
struct TwStatus {
    condition: u16,
    data: u16,
}

#[repr(C, packed(2))]
#[derive(Default)]
struct TwImageInfo {
    x_resolution: u32,
    y_resolution: u32,
    width: i32,
    length: i32,
    samples: i16,
    bits_per_sample: [i16; 8],
    bits_per_pixel: i16,
    planar: u16,
    pixel_type: i16,
    compression: u16,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct TwInfo {
    id: u16,
    item_type: u16,
    items: u16,
    condition: u16,
    item: usize,
}

#[repr(C, packed(2))]
struct TwExtImageInfo {
    count: u32,
    info: [TwInfo; EXT_INFO_COUNT],
}

#[repr(C, packed(2))]
struct TwDeviceEvent {
    event: u32,
    device_name: [u8; 256],
    battery_minutes: u32,
    battery_percentage: i16,
    power_supply: i32,
    x_resolution: u32,
    y_resolution: u32,
    flash_used: u32,
    automatic_capture: u32,
    time_before_first_capture: u32,
    time_between_captures: u32,
}

/// TWAIN accepts event processing only while the data source is enabled.
fn routes_events_to_driver(state: u8) -> bool {
    state >= 5
}

fn device_event_name(event: u32) -> &'static str {
    match event {
        7 => "device offline",
        8 => "device ready",
        9 => "device removed",
        12 => "double feed",
        13 => "paper jam",
        _ => "not mapped yet",
    }
}

fn device_event_fault(event: u32) -> Option<StopReason> {
    match event {
        7 | 9 => Some(StopReason::DeviceFailure),
        12 => Some(StopReason::DoubleFeed),
        13 => Some(StopReason::Jam),
        _ => None,
    }
}

fn twain_condition_name(condition: u16) -> &'static str {
    match condition {
        20 => "paper jam",
        21 => "double feed",
        23 => "device offline",
        29 => "no media",
        _ => "not mapped yet",
    }
}

fn pending_label(count: u16) -> String {
    if count == u16::MAX {
        "more".to_owned()
    } else {
        count.to_string()
    }
}

fn image_tag_name(id: u16) -> String {
    match id {
        0x123c => "camera".to_owned(),
        0x123e => "frame".to_owned(),
        0x1240 => "ICC profile".to_owned(),
        0x1241 => "last segment".to_owned(),
        0x1242 => "segment number".to_owned(),
        0x1243 => "magnetic data".to_owned(),
        0x1244 => "magnetic type".to_owned(),
        0x1246 => "file source".to_owned(),
        0x1247 => "image merged".to_owned(),
        0x1248 => "magnetic data length".to_owned(),
        0x124a => "printer text".to_owned(),
        other => format!("driver tag {other:#06x}"),
    }
}

/// Keeps only metadata that tells the user something about this image.
fn image_tag_note(id: u16, value: &str, side: Side) -> Option<String> {
    match id {
        0x1238..=0x123b | 0x123d | 0x123f | 0x1249 => None,
        0x1245 => {
            let reported = match value {
                "1" => Side::Front,
                "2" => Side::Back,
                other => return Some(format!("driver page side {other}")),
            };
            (reported != side).then(|| format!("driver reports {reported:?} side"))
        }
        _ => Some(format!("{} = {value}", image_tag_name(id))),
    }
}

/// Formats one explainable log line per scanned side.
fn sheet_log_lines(sides: &[(SavedSide, String)], dpi: u32, pending: &str) -> Vec<String> {
    let last = sides.len().saturating_sub(1);
    sides
        .iter()
        .enumerate()
        .map(|(index, (side, note))| {
            let mut line = format!(
                "sheet {:04} {:<5}: {}x{} px, {:.0}x{:.0} mm, {:.2} MB, {}",
                side.sheet,
                format!("{:?}", side.side),
                side.width,
                side.height,
                f64::from(side.width) / f64::from(dpi) * 25.4,
                f64::from(side.height) / f64::from(dpi) * 25.4,
                side.bytes as f64 / 1_048_576.0,
                side.sha256.get(..12).unwrap_or("")
            );
            if side.status == SideStatus::Partial {
                line.push_str(", PARTIAL");
            }
            if let Some(stop) = side.stop {
                line.push_str(&format!(", stopped: {}", stop.name()));
            }
            if !note.is_empty() {
                line.push_str(&format!(", {note}"));
            }
            if let Some(timing) = side.timing.label() {
                line.push_str(&format!(", {timing}"));
            }
            if index == last && !pending.is_empty() {
                line.push_str(&format!(", pending {pending}"));
            }
            line
        })
        .collect()
}

fn flush_sheet_log(sheet: &mut Vec<(SavedSide, String)>, dpi: u32, pending: &str) {
    for line in sheet_log_lines(sheet, dpi, pending) {
        trace("scan", line);
    }
    sheet.clear();
}

#[repr(C, packed(2))]
struct TwMemory {
    flags: u32,
    length: u32,
    memory: *mut u8,
}

#[repr(C, packed(2))]
struct TwImageMemXfer {
    compression: u16,
    bytes_per_row: u32,
    columns: u32,
    rows: u32,
    x_offset: u32,
    y_offset: u32,
    bytes_written: u32,
    memory: TwMemory,
}

#[repr(C)]
struct WinMsg {
    window: Handle,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    x: i32,
    y: i32,
    private: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WinRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct WinPoint {
    x: i32,
    y: i32,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    paint: WinRect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_ppm: i32,
    y_ppm: i32,
    colors_used: u32,
    colors_important: u32,
}

type WindowProc = unsafe extern "system" fn(Handle, u32, usize, isize) -> isize;

#[repr(C)]
struct WndClassW {
    style: u32,
    proc_: Option<WindowProc>,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GetModuleHandleW(name: *const u16) -> Handle;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn DefWindowProcW(window: Handle, message: u32, w: usize, l: isize) -> isize;
    fn GetMessageW(message: *mut WinMsg, window: Handle, min: u32, max: u32) -> i32;
    fn PeekMessageW(message: *mut WinMsg, window: Handle, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(message: *const WinMsg) -> i32;
    fn DispatchMessageW(message: *const WinMsg) -> isize;
    fn PostQuitMessage(code: i32);
    fn SetTimer(window: Handle, id: usize, elapse: u32, callback: Handle) -> usize;
    fn BeginPaint(window: Handle, paint: *mut PaintStruct) -> Handle;
    fn EndPaint(window: Handle, paint: *const PaintStruct) -> i32;
    fn InvalidateRect(window: Handle, rect: *const WinRect, erase: i32) -> i32;
    fn GetClientRect(window: Handle, rect: *mut WinRect) -> i32;
    fn FillRect(hdc: Handle, rect: *const WinRect, brush: Handle) -> i32;
    fn SendMessageW(window: Handle, message: u32, w: usize, l: isize) -> isize;
    fn DrawTextW(hdc: Handle, text: *const u16, count: i32, rect: *mut WinRect, format: u32)
    -> i32;
    fn MoveWindow(window: Handle, x: i32, y: i32, width: i32, height: i32, repaint: i32) -> i32;
    fn GetWindowTextLengthW(window: Handle) -> i32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn SetCursor(cursor: Handle) -> Handle;
    fn SetCapture(window: Handle) -> Handle;
    fn ReleaseCapture() -> i32;
    fn ScreenToClient(window: Handle, point: *mut WinPoint) -> i32;

}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn StretchDIBits(
        hdc: Handle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source_x: i32,
        source_y: i32,
        source_width: i32,
        source_height: i32,
        bits: *const u8,
        info: *const BitmapInfoHeader,
        usage: u32,
        rop: u32,
    ) -> i32;
    fn SetStretchBltMode(hdc: Handle, mode: i32) -> i32;
    fn SetBkMode(hdc: Handle, mode: i32) -> i32;
    fn SetTextColor(hdc: Handle, color: u32) -> u32;
    fn SetBkColor(hdc: Handle, color: u32) -> u32;
    fn CreateSolidBrush(color: u32) -> Handle;
    fn GetStockObject(index: i32) -> Handle;
    fn CreatePen(style: i32, width: i32, color: u32) -> Handle;
    fn MoveToEx(hdc: Handle, x: i32, y: i32, previous: *mut WinPoint) -> i32;
    fn LineTo(hdc: Handle, x: i32, y: i32) -> i32;
    fn Polygon(hdc: Handle, points: *const WinPoint, count: i32) -> i32;
    fn SaveDC(hdc: Handle) -> i32;
    fn RestoreDC(hdc: Handle, saved: i32) -> i32;
    fn IntersectClipRect(hdc: Handle, left: i32, top: i32, right: i32, bottom: i32) -> i32;
    fn CreateCompatibleDC(hdc: Handle) -> Handle;
    fn CreateCompatibleBitmap(hdc: Handle, width: i32, height: i32) -> Handle;
    fn SelectObject(hdc: Handle, object: Handle) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn DeleteDC(hdc: Handle) -> i32;
    fn BitBlt(
        target: Handle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source: Handle,
        source_x: i32,
        source_y: i32,
        rop: u32,
    ) -> i32;
    fn TextOutW(hdc: Handle, x: i32, y: i32, text: *const u16, count: i32) -> i32;
}

/// Generic TWAIN adapter. It must stay on the thread that created it.
struct TwainScanner {
    entry: DsmEntry,
    module: Handle,
    window: Handle,
    app: TwIdentity,
    source: TwIdentity,
    state: u8,
    shared: Arc<ScanShared>,
    color: ColorMode,
    dpi: u32,
    duplex: bool,
    next: Side,
    width: usize,
    buffer: Vec<u8>,
    bytes_per_row: usize,
    block_rows: usize,
    block_row: usize,
    transfer_done: bool,
    needs_reload: bool,
    loaded_polls: u8,
    last_loaded: Option<bool>,
    no_media_retries: u8,
    device_fault: Option<StopReason>,
    device_event_failed: bool,
    awaiting_clear: bool,
    scanner_ready: bool,
    preferred: Option<String>,
    selected: Option<String>,
    config_path: Option<PathBuf>,
    device: Option<&'static DeviceProfile>,
    stop_feeder_sent: bool,
    pause_polls: u32,
    pickup_failed_clean: bool,
}

/// A front side that failed with no paper and no pixels lost nothing if the feeder is now empty.
fn failed_before_any_image(
    reason: StopReason,
    bytes_per_row: usize,
    duplex: bool,
    next: Side,
) -> bool {
    let failed_on_front = !duplex || next == Side::Back;
    reason == StopReason::NoMedia && bytes_per_row == 0 && failed_on_front
}

impl TwainScanner {
    /// Opens the scanner, waiting visibly while another application holds it.
    fn open(
        shared: Arc<ScanShared>,
        preferred: Option<String>,
        config_path: Option<PathBuf>,
    ) -> io::Result<Self> {
        // SAFETY: pointers passed to Win32 and TWAIN outlive each call, and ownership
        // of opened handles moves into the returned scanner.
        unsafe {
            let module = LoadLibraryW(wide("TWAINDSM.dll").as_ptr());
            if module.is_null() {
                return Err(io::Error::last_os_error());
            }
            let address = GetProcAddress(module, c"DSM_Entry".as_ptr().cast());
            if address.is_null() {
                FreeLibrary(module);
                return Err(io::Error::other("TWAINDSM.dll has no DSM_Entry"));
            }
            let entry: DsmEntry = std::mem::transmute(address);
            let mut window = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("OwnScanner TWAIN").as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            if window.is_null() {
                FreeLibrary(module);
                return Err(io::Error::last_os_error());
            }
            let mut app = app_identity();
            let opened = entry(
                &mut app,
                std::ptr::null_mut(),
                DG_CONTROL,
                DAT_PARENT,
                MSG_OPENDSM,
                (&raw mut window).cast(),
            );
            if opened != TWRC_SUCCESS {
                DestroyWindow(window);
                FreeLibrary(module);
                return Err(io::Error::other(format!(
                    "cannot open TWAIN manager: {opened}"
                )));
            }
            let mut scanner = Self {
                entry,
                module,
                window,
                app,
                source: empty_identity(),
                state: 3,
                shared,
                color: ColorMode::Rgb24,
                dpi: 600,
                duplex: true,
                next: Side::Front,
                width: 0,
                buffer: Vec::new(),
                bytes_per_row: 0,
                block_rows: 0,
                block_row: 0,
                transfer_done: false,
                needs_reload: false,
                loaded_polls: 0,
                last_loaded: None,
                no_media_retries: 0,
                device_fault: None,
                device_event_failed: false,
                awaiting_clear: false,
                scanner_ready: true,
                preferred,
                selected: None,
                config_path,
                device: None,
                stop_feeder_sent: false,
                pause_polls: 0,
                pickup_failed_clean: false,
            };
            scanner.acquire_lock()?;
            Ok(scanner)
        }
    }

    fn acquire_lock(&mut self) -> io::Result<()> {
        loop {
            if self.shared.stopped() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
            }
            match self.select_source()? {
                None => self.shared.show_once(
                    "Waiting for a scanner",
                    "No TWAIN scanner was found. Detection continues automatically.",
                ),
                Some(mut source) => {
                    let name = tw_string(&{ source.product_name });
                    let device = device_profile(&tw_string(&{ source.manufacturer }), &name);
                    let holder = device.and_then(|device| running_holder(device.lock_holders));
                    if let Some(holder) = holder {
                        self.shared.show_once(
                            "Waiting for scanner lock",
                            format!("Close {holder}. Scanning will start automatically."),
                        );
                    } else if self.open_source(&mut source) {
                        self.source = source;
                        self.device = device;
                        trace("scanner", format!("lock acquired on {name}"));
                        trace(
                            "scanner",
                            format!(
                                "device profile: {}",
                                device.map_or("generic TWAIN", |device| device.name)
                            ),
                        );
                        self.state = 4;
                        return Ok(());
                    } else {
                        self.shared.show_once(
                            "Waiting for scanner lock",
                            format!("{name} is busy or offline. Retrying automatically."),
                        );
                    }
                }
            }
            self.shared.set_phase(ScanPhase::WaitingForLock);
            thread::sleep(Duration::from_secs(1));
        }
    }

    fn select_source(&mut self) -> io::Result<Option<TwIdentity>> {
        let (sources, system_default) = twain_sources(self.entry, &mut self.app);
        let find = |wanted: &str| {
            sources
                .iter()
                .copied()
                .find(|source| tw_string(&{ source.product_name }) == wanted)
        };
        if let Some(name) = &self.selected {
            return Ok(find(name));
        }
        let names = sources
            .iter()
            .map(|source| tw_string(&{ source.product_name }))
            .collect::<Vec<_>>();
        match choose_source(&names, self.preferred.as_deref(), system_default.as_deref()) {
            SourceChoice::Use { name, reason } => {
                trace(
                    "scanner",
                    format!("found: {}; using {name} ({reason})", names.join(", ")),
                );
                let source = find(&name);
                self.selected = Some(name);
                Ok(source)
            }
            SourceChoice::Ask { reason } => {
                trace(
                    "scanner",
                    format!("found: {}; asking because {reason}", names.join(", ")),
                );
                self.shared.set_phase(ScanPhase::WaitingForLock);
                self.shared.show(
                    "Choose a scanner",
                    format!("{reason}. Select one in the dialog."),
                );
                let mut chosen = sources.first().copied().unwrap_or_else(empty_identity);
                // SAFETY: `chosen` is a valid TW_IDENTITY for the source manager dialog.
                let result = unsafe {
                    (self.entry)(
                        &mut self.app,
                        std::ptr::null_mut(),
                        DG_CONTROL,
                        DAT_IDENTITY,
                        MSG_USERSELECT,
                        (&raw mut chosen).cast(),
                    )
                };
                if result != TWRC_SUCCESS {
                    return Err(io::Error::other("no scanner was selected"));
                }
                let name = tw_string(&{ chosen.product_name });
                trace("scanner", format!("user selected {name}"));
                if let Some(path) = &self.config_path {
                    match remember_scanner(path, &name) {
                        Ok(()) => {
                            trace("config", format!("remembered {name} in {}", path.display()))
                        }
                        Err(error) => {
                            trace("warning", format!("could not remember scanner: {error}"))
                        }
                    }
                }
                self.selected = Some(name);
                Ok(Some(chosen))
            }
            SourceChoice::None => Ok(None),
        }
    }

    fn open_source(&mut self, source: &mut TwIdentity) -> bool {
        // SAFETY: `source` is a valid TW_IDENTITY returned by the source manager.
        unsafe {
            (self.entry)(
                &mut self.app,
                std::ptr::null_mut(),
                DG_CONTROL,
                DAT_IDENTITY,
                MSG_OPENDS,
                (source as *mut TwIdentity).cast(),
            ) == TWRC_SUCCESS
        }
    }
    fn ds(
        &mut self,
        group: u32,
        data_type: u16,
        message: u16,
        data: *mut core::ffi::c_void,
    ) -> u16 {
        // SAFETY: callers pass a pointer to the TWAIN structure required by the triplet.
        unsafe {
            (self.entry)(
                &mut self.app,
                &mut self.source,
                group,
                data_type,
                message,
                data,
            )
        }
    }

    fn condition(&mut self) -> u16 {
        let mut status = TwStatus::default();
        self.ds(DG_CONTROL, DAT_STATUS, MSG_GET, (&raw mut status).cast());
        status.condition
    }

    fn set_cap(&mut self, cap: u16, item_type: u16, value: u32) -> bool {
        // SAFETY: the allocated block is large enough for TW_ONEVALUE and is unlocked
        // before TWAIN reads it.
        unsafe {
            let handle = GlobalAlloc(0x42, 16);
            if handle.is_null() {
                return false;
            }
            let memory = GlobalLock(handle);
            if memory.is_null() {
                GlobalFree(handle);
                return false;
            }
            (memory as *mut u16).write_unaligned(item_type);
            (memory.add(2) as *mut u32).write_unaligned(value);
            GlobalUnlock(handle);
            let mut request = TwCapability {
                cap,
                container_type: 5,
                container: handle,
            };
            let result = self.ds(
                DG_CONTROL,
                DAT_CAPABILITY,
                MSG_SET,
                (&raw mut request).cast(),
            );
            GlobalFree(handle);
            result == TWRC_SUCCESS || result == TWRC_CHECKSTATUS
        }
    }

    fn set_cap_array(&mut self, cap: u16, values: &[u16]) -> bool {
        // SAFETY: the block holds a packed TW_ARRAY of UINT16 values and is unlocked
        // before TWAIN reads it.
        unsafe {
            let handle = GlobalAlloc(0x42, 6 + values.len() * 2);
            if handle.is_null() {
                return false;
            }
            let memory = GlobalLock(handle);
            if memory.is_null() {
                GlobalFree(handle);
                return false;
            }
            (memory as *mut u16).write_unaligned(TWTY_UINT16);
            (memory.add(2) as *mut u32).write_unaligned(values.len() as u32);
            for (index, value) in values.iter().enumerate() {
                (memory.add(6 + index * 2) as *mut u16).write_unaligned(*value);
            }
            GlobalUnlock(handle);
            let mut request = TwCapability {
                cap,
                container_type: 3,
                container: handle,
            };
            let result = self.ds(
                DG_CONTROL,
                DAT_CAPABILITY,
                MSG_SET,
                (&raw mut request).cast(),
            );
            GlobalFree(handle);
            result == TWRC_SUCCESS || result == TWRC_CHECKSTATUS
        }
    }

    fn read_device_event(&mut self) {
        // SAFETY: TW_DEVICEEVENT contains only integers and bytes, so zero is valid.
        let mut event: TwDeviceEvent = unsafe { std::mem::zeroed() };
        if self.ds(
            DG_CONTROL,
            DAT_DEVICEEVENT,
            MSG_GET,
            (&raw mut event).cast(),
        ) != TWRC_SUCCESS
        {
            if !self.device_event_failed {
                self.device_event_failed = true;
                let condition = self.condition();
                trace(
                    "warning",
                    format!(
                        "device event could not be read; TWAIN condition {condition} ({})",
                        twain_condition_name(condition)
                    ),
                );
            }
            return;
        }
        self.device_event_failed = false;
        let id = { event.event };
        trace("device", format!("event {id} ({})", device_event_name(id)));
        if let Some(reason) = device_event_fault(id) {
            self.device_fault = Some(reason);
        } else if id == u32::from(TWDE_DEVICEREADY) && self.needs_reload {
            self.needs_reload = false;
            self.loaded_polls = 0;
            trace(
                "decision",
                "scanner reports ready; resuming without a button",
            );
        }
    }

    fn cap_current(&mut self, cap: u16) -> Option<String> {
        let mut request = TwCapability {
            cap,
            container_type: 0xffff,
            container: std::ptr::null_mut(),
        };
        if self.ds(
            DG_CONTROL,
            DAT_CAPABILITY,
            MSG_GETCURRENT,
            (&raw mut request).cast(),
        ) != TWRC_SUCCESS
        {
            return None;
        }
        let handle = request.container;
        // SAFETY: a successful capability request returns a global-memory container.
        unsafe {
            let memory = GlobalLock(handle);
            let value = (!memory.is_null()).then(|| twain_values(request.container_type, memory).1);
            GlobalUnlock(handle);
            GlobalFree(handle);
            value
        }
    }

    fn cap_options(&mut self, cap: u16) -> String {
        let mut request = TwCapability {
            cap,
            container_type: 0xffff,
            container: std::ptr::null_mut(),
        };
        if self.ds(
            DG_CONTROL,
            DAT_CAPABILITY,
            MSG_GET,
            (&raw mut request).cast(),
        ) != TWRC_SUCCESS
        {
            return "unavailable".to_owned();
        }
        let handle = request.container;
        // SAFETY: a successful capability request returns a global-memory container.
        unsafe {
            let memory = GlobalLock(handle);
            let values = if memory.is_null() {
                "unavailable".to_owned()
            } else {
                twain_values(request.container_type, memory).0.join(", ")
            };
            GlobalUnlock(handle);
            GlobalFree(handle);
            values
        }
    }

    fn feeder_loaded(&mut self) -> bool {
        let loaded = self.cap_current(CAP_FEEDERLOADED).as_deref() == Some("true");
        if self.last_loaded != Some(loaded) {
            self.last_loaded = Some(loaded);
            trace(
                "feeder",
                if loaded {
                    "paper detected"
                } else {
                    "feeder is empty"
                },
            );
        }
        loaded
    }

    fn enable(&mut self) -> io::Result<()> {
        let mut ui = TwUserInterface {
            show_ui: 0,
            modal_ui: 0,
            parent: self.window,
        };
        let result = self.ds(
            DG_CONTROL,
            DAT_USERINTERFACE,
            MSG_ENABLEDS,
            (&raw mut ui).cast(),
        );
        if result != TWRC_SUCCESS && result != TWRC_CHECKSTATUS {
            let condition = self.condition();
            return Err(io::Error::other(format!(
                "scanner refused to start: return {result}, condition {condition}"
            )));
        }
        self.state = 5;
        Ok(())
    }

    fn disable(&mut self) {
        if self.state == 5 {
            let mut ui = TwUserInterface {
                show_ui: 0,
                modal_ui: 0,
                parent: self.window,
            };
            self.ds(
                DG_CONTROL,
                DAT_USERINTERFACE,
                MSG_DISABLEDS,
                (&raw mut ui).cast(),
            );
            self.state = 4;
        }
    }

    fn pump(&mut self) -> Option<u16> {
        let mut message: WinMsg = unsafe { std::mem::zeroed() };
        if !routes_events_to_driver(self.state) {
            // SAFETY: `message` is a valid MSG for the standard message APIs.
            unsafe {
                while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, 1) != 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            return None;
        }
        // SAFETY: `message` is a valid MSG for the message APIs and TWAIN event call.
        unsafe {
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, 1) != 0 {
                let mut event = TwEvent {
                    event: &mut message,
                    message: 0,
                };
                let result = self.ds(
                    DG_CONTROL,
                    DAT_EVENT,
                    MSG_PROCESSEVENT,
                    (&raw mut event).cast(),
                );
                let notice = event.message;
                if notice == MSG_DEVICEEVENT {
                    self.read_device_event();
                    continue;
                }
                if notice == MSG_XFERREADY || notice == MSG_CLOSEDSREQ || notice == MSG_CLOSEDSOK {
                    return Some(notice);
                }
                if result != TWRC_DSEVENT {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
        None
    }

    /// TWAIN first accepts a feeder stop when an image is ready, before that image is transferred.
    fn stop_feeder_before_transfer(&mut self) {
        if !self.shared.paused.load(Ordering::Relaxed) || self.stop_feeder_sent {
            return;
        }
        let mut pending = TwPendingXfers::default();
        let result = self.ds(
            DG_CONTROL,
            DAT_PENDINGXFERS,
            MSG_STOPFEEDER,
            (&raw mut pending).cast(),
        );
        if result == TWRC_SUCCESS {
            self.stop_feeder_sent = true;
            trace(
                "decision",
                format!(
                    "pause: feeder stopped before transfer; {} images still pending",
                    pending_label(pending.count)
                ),
            );
        } else {
            let condition = self.condition();
            trace(
                "decision",
                format!(
                    "pause: early feeder stop refused, TWAIN condition {condition} ({}); retrying after this image",
                    twain_condition_name(condition)
                ),
            );
        }
    }

    fn begin_image(&mut self) -> io::Result<DeviceStep> {
        self.stop_feeder_before_transfer();
        let mut image = TwImageInfo::default();
        if self.ds(DG_IMAGE, DAT_IMAGEINFO, MSG_GET, (&raw mut image).cast()) != TWRC_SUCCESS {
            return Err(io::Error::other("scanner did not report image information"));
        }
        let mut setup = TwSetupMemXfer::default();
        if self.ds(
            DG_CONTROL,
            DAT_SETUPMEMXFER,
            MSG_GET,
            (&raw mut setup).cast(),
        ) != TWRC_SUCCESS
        {
            return Err(io::Error::other("scanner did not report transfer buffers"));
        }
        // The image-info call can block for seconds while the driver reads the page.
        self.stop_feeder_before_transfer();
        let preferred = { setup.preferred };
        let size = if preferred == 0 || preferred == u32::MAX {
            1024 * 1024
        } else {
            preferred as usize
        };
        self.buffer.resize(size.max(64 * 1024), 0);
        self.width = { image.width } as usize;
        (self.block_rows, self.block_row, self.bytes_per_row) = (0, 0, 0);
        self.transfer_done = false;
        let side = self.next;
        if self.duplex {
            self.next = if side == Side::Front {
                Side::Back
            } else {
                Side::Front
            };
        }
        let index = usize::from(side == Side::Back);
        {
            let mut ui = self.shared.ui.lock().expect("ui lock");
            ui.panes[index].begin(self.width, format!("{side:?}"));
            ui.version += 1;
        }
        Ok(DeviceStep::Side(SideInfo {
            side,
            width: self.width as u32,
            dpi: self.dpi,
            color: self.color,
        }))
    }

    /// Reads every scalar metadata tag the driver reports for the finished image.
    fn read_image_tags(&mut self) -> String {
        let mut request = Box::new(TwExtImageInfo {
            count: EXT_INFO_COUNT as u32,
            info: [TwInfo {
                id: 0,
                item_type: 0,
                items: 0,
                condition: 0,
                item: 0,
            }; EXT_INFO_COUNT],
        });
        for (offset, info) in request.info.iter_mut().enumerate() {
            info.id = EXT_INFO_FIRST + offset as u16;
        }
        let pointer = (&raw mut *request).cast();
        if self.ds(DG_IMAGE, DAT_EXTIMAGEINFO, MSG_GET, pointer) != TWRC_SUCCESS {
            return String::new();
        }
        let side = if self.duplex && self.next == Side::Front {
            Side::Back
        } else {
            Side::Front
        };
        let mut tags = Vec::new();
        for info in request.info {
            let (id, item_type, items, condition, item) = (
                info.id,
                info.item_type,
                info.items,
                info.condition,
                info.item,
            );
            if condition != TWRC_SUCCESS || items == 0 {
                continue;
            }
            let inline = item_type <= TWTY_FIX32
                && usize::from(items) * twain_item_size(item_type) <= std::mem::size_of::<usize>();
            let value = if inline && items == 1 {
                let bytes = item.to_le_bytes();
                // SAFETY: `bytes` holds the inline TWAIN item selected by `item_type`.
                unsafe { twain_item(bytes.as_ptr(), item_type) }
            } else {
                if !inline && item != 0 {
                    // SAFETY: TWAIN returns non-inline metadata as an owned global handle.
                    unsafe { GlobalFree(item as Handle) };
                }
                format!("{items} items")
            };
            if let Some(note) = image_tag_note(id, &value, side) {
                tags.push(note);
            }
        }
        tags.join("; ")
    }

    fn end_transfer(&mut self, tags: String) {
        let mut pending = TwPendingXfers::default();
        self.ds(
            DG_CONTROL,
            DAT_PENDINGXFERS,
            MSG_ENDXFER,
            (&raw mut pending).cast(),
        );
        {
            let mut ui = self.shared.ui.lock().expect("ui lock");
            ui.driver_pending = pending_label(pending.count);
            ui.driver_tags = tags;
        }
        self.no_media_retries = 0;
        self.pickup_failed_clean = false;
        if self.shared.paused.load(Ordering::Relaxed)
            && { pending.count } != 0
            && !self.stop_feeder_sent
        {
            self.stop_feeder_sent = true;
            let result = self.ds(
                DG_CONTROL,
                DAT_PENDINGXFERS,
                MSG_STOPFEEDER,
                (&raw mut pending).cast(),
            );
            if result == TWRC_SUCCESS {
                trace(
                    "decision",
                    format!(
                        "pause: feeder stopped; {} images still pending",
                        pending_label(pending.count)
                    ),
                );
            } else {
                let condition = self.condition();
                trace(
                    "warning",
                    format!(
                        "pause: driver refused to stop the feeder; TWAIN condition {condition} ({}). Saving remaining images.",
                        twain_condition_name(condition)
                    ),
                );
            }
        }
        if { pending.count } == 0 {
            self.state = 5;
            self.disable();
        } else {
            self.state = 6;
        }
    }

    fn cancel(&mut self) {
        if self.state >= 6 {
            let mut pending = TwPendingXfers::default();
            if self.state == 7 {
                self.ds(
                    DG_CONTROL,
                    DAT_PENDINGXFERS,
                    MSG_ENDXFER,
                    (&raw mut pending).cast(),
                );
            }
            self.ds(
                DG_CONTROL,
                DAT_PENDINGXFERS,
                MSG_RESET,
                (&raw mut pending).cast(),
            );
            self.state = 5;
        }
        self.disable();
        self.next = Side::Front;
    }

    fn fail_transfer(&mut self) -> RowStep {
        let condition = self.condition();
        trace(
            "scanner",
            format!(
                "transfer failed; TWAIN condition {condition} ({})",
                twain_condition_name(condition)
            ),
        );
        let reason = match condition {
            20 => StopReason::Jam,
            21 => StopReason::DoubleFeed,
            23 => StopReason::DeviceFailure,
            29 => StopReason::NoMedia,
            _ => StopReason::PaperFeedError,
        };
        self.pickup_failed_clean =
            failed_before_any_image(reason, self.bytes_per_row, self.duplex, self.next);
        self.cancel();
        if reason == StopReason::NoMedia && self.no_media_retries < 3 {
            self.no_media_retries += 1;
            trace(
                "decision",
                format!(
                    "paper was not picked up; automatic retry {} of 3",
                    self.no_media_retries
                ),
            );
        } else {
            if reason == StopReason::NoMedia {
                trace(
                    "decision",
                    "paper was still not picked up after 3 retries; waiting for it to be fixed or removed",
                );
            }
            self.no_media_retries = 0;
            self.needs_reload = true;
            self.awaiting_clear = true;
            self.scanner_ready = true;
        }
        RowStep::Stopped(reason)
    }

    fn publish_preview(&self, index: usize) {
        let mut ui = self.shared.ui.lock().expect("ui lock");
        ui.panes[index].publish();
        ui.version += 1;
    }

    fn read_block(&mut self) -> Option<RowStep> {
        let mut transfer = TwImageMemXfer {
            compression: 0xffff,
            bytes_per_row: u32::MAX,
            columns: u32::MAX,
            rows: u32::MAX,
            x_offset: u32::MAX,
            y_offset: u32::MAX,
            bytes_written: u32::MAX,
            memory: TwMemory {
                flags: 0x0001 | 0x0008,
                length: self.buffer.len() as u32,
                memory: self.buffer.as_mut_ptr(),
            },
        };
        let result = self.ds(
            DG_IMAGE,
            DAT_IMAGEMEMXFER,
            MSG_GET,
            (&raw mut transfer).cast(),
        );
        match result {
            TWRC_SUCCESS | TWRC_XFERDONE => {
                self.state = 7;
                self.bytes_per_row = { transfer.bytes_per_row } as usize;
                self.block_rows = { transfer.rows } as usize;
                self.block_row = 0;
                self.transfer_done = result == TWRC_XFERDONE;
                None
            }
            TWRC_CANCEL => {
                self.cancel();
                self.needs_reload = true;
                Some(RowStep::Stopped(StopReason::PaperFeedError))
            }
            _ => {
                self.state = 7;
                Some(self.fail_transfer())
            }
        }
    }
}

impl ScannerDevice for TwainScanner {
    fn instruction(&self, reason: StopReason) -> Option<String> {
        self.device
            .and_then(|device| device.help(reason))
            .map(str::to_owned)
    }

    fn identity(&self) -> String {
        format!("{} (TWAIN)", tw_string(&{ self.source.product_name }))
    }

    fn configure(&mut self, profile: &Profile) -> io::Result<()> {
        (self.color, self.dpi, self.duplex) = (profile.color, profile.dpi, profile.duplex);
        let (pixel, depth) = match profile.color {
            ColorMode::Rgb24 => (2, 8),
            ColorMode::Gray8 => (1, 8),
        };
        let required = [
            (ICAP_XFERMECH, TWTY_UINT16, 2, "memory transfer"),
            (ICAP_COMPRESSION, TWTY_UINT16, 0, "no compression"),
            (ICAP_PIXELTYPE, TWTY_UINT16, pixel, "color mode"),
            (ICAP_UNITS, TWTY_UINT16, 0, "inches"),
            (ICAP_XRESOLUTION, TWTY_FIX32, profile.dpi, "x resolution"),
            (ICAP_YRESOLUTION, TWTY_FIX32, profile.dpi, "y resolution"),
            (CAP_FEEDERENABLED, TWTY_BOOL, 1, "feeder"),
            (
                CAP_DUPLEXENABLED,
                TWTY_BOOL,
                u32::from(profile.duplex),
                "duplex",
            ),
            (ICAP_UNDEFINEDIMAGESIZE, TWTY_BOOL, 1, "undefined length"),
        ];
        for (cap, item_type, value, name) in required {
            if !self.set_cap(cap, item_type, value) {
                let condition = self.condition();
                let supported = self.cap_options(cap);
                return Err(io::Error::other(format!(
                    "scanner rejected {name}: TWAIN condition {condition} ({}); driver supports: {supported}",
                    twain_condition_name(condition)
                )));
            }
        }
        for (cap, name) in [
            (ICAP_XRESOLUTION, "x resolution"),
            (ICAP_YRESOLUTION, "y resolution"),
        ] {
            let current = self.cap_current(cap).unwrap_or_default();
            let matches = current
                .parse::<f64>()
                .is_ok_and(|value| (value - f64::from(profile.dpi)).abs() < 0.5);
            if !matches {
                let supported = self.cap_options(cap);
                return Err(io::Error::other(format!(
                    "scanner uses {name} {current} instead of {} dpi; driver supports: {supported}",
                    profile.dpi
                )));
            }
        }
        let total_depth = depth * profile.color.channels() as u32;
        if !self.set_cap(ICAP_BITDEPTH, TWTY_UINT16, depth)
            && !self.set_cap(ICAP_BITDEPTH, TWTY_UINT16, total_depth)
        {
            let current = self
                .cap_current(ICAP_BITDEPTH)
                .unwrap_or_else(|| "unavailable".into());
            if current != depth.to_string() && current != total_depth.to_string() {
                return Err(io::Error::other(format!(
                    "scanner rejected bit depth; current value is {current}"
                )));
            }
        }
        let optional = [
            (
                ICAP_SUPPORTEDSIZES,
                TWTY_UINT16,
                TWSS_NONE,
                "page-size preset (0=none)",
            ),
            (CAP_INDICATORS, TWTY_BOOL, 0, "driver progress dialog"),
            (ICAP_EXTIMAGEINFO, TWTY_BOOL, 1, "image metadata"),
            (CAP_XFERCOUNT, TWTY_INT16, 0xffff, "transfer count"),
            (CAP_AUTOFEED, TWTY_BOOL, 1, "auto feed"),
            // Read-ahead is faster, but sheets already read are still delivered after Pause.
            (
                CAP_AUTOSCAN,
                TWTY_BOOL,
                u32::from(profile.read_ahead),
                "read-ahead scanning",
            ),
            (ICAP_PIXELFLAVOR, TWTY_UINT16, 0, "pixel flavor"),
            (ICAP_PLANARCHUNKY, TWTY_UINT16, 0, "chunky pixels"),
            (
                ICAP_AUTOMATICBORDERDETECTION,
                TWTY_BOOL,
                1,
                "page size detection",
            ),
            (
                ICAP_AUTOMATICDESKEW,
                TWTY_BOOL,
                u32::from(profile.deskew),
                "deskew",
            ),
            (ICAP_AUTOMATICROTATE, TWTY_BOOL, 0, "auto rotate"),
            (
                ICAP_AUTODISCARDBLANKPAGES,
                TWTY_INT32,
                (-2i32) as u32,
                "blank discard",
            ),
        ];
        let mut summary = Vec::new();
        for (cap, item_type, value, name) in optional {
            let accepted = self.set_cap(cap, item_type, value);
            let current = self
                .cap_current(cap)
                .unwrap_or_else(|| "unavailable".into());
            if !accepted {
                trace(
                    "warning",
                    format!("driver rejected {name}; it reports {current}"),
                );
            }
            summary.push(format!("{name}={current}"));
        }
        if let Some(cap) = self.device.and_then(|device| device.long_document_cap) {
            let entry = self.entry;
            match apply_long_document_mode(
                entry,
                &mut self.app,
                &mut self.source,
                cap,
                profile.max_length_mm,
            ) {
                Some(mode) => summary.push(format!("long-document mode mm={mode}")),
                None => trace("warning", "driver rejected long-document mode"),
            }
        }
        for (cap, name) in [
            (ICAP_PIXELTYPE, "pixel type"),
            (ICAP_XRESOLUTION, "x resolution"),
            (ICAP_YRESOLUTION, "y resolution"),
            (CAP_DUPLEXENABLED, "duplex"),
            (ICAP_UNDEFINEDIMAGESIZE, "undefined length"),
            (ICAP_AUTOSIZE, "auto size"),
            (ICAP_PHYSICALWIDTH, "driver max width in"),
            (ICAP_PHYSICALHEIGHT, "driver max height in"),
        ] {
            let current = self
                .cap_current(cap)
                .unwrap_or_else(|| "unavailable".into());
            summary.push(format!("{name}={current}"));
        }
        let subscribed = self.set_cap_array(
            CAP_DEVICEEVENT,
            &[
                TWDE_DEVICEOFFLINE,
                TWDE_DEVICEREADY,
                TWDE_DEVICEREMOVED,
                TWDE_PAPERDOUBLEFEED,
                TWDE_PAPERJAM,
            ],
        );
        if !subscribed {
            let condition = self.condition();
            trace(
                "warning",
                format!("driver rejected device events; condition {condition}"),
            );
        }
        summary.push(format!("device events={subscribed}"));
        trace("settings", format!("for this run: {}", summary.join(", ")));
        Ok(())
    }

    fn next_side(&mut self) -> io::Result<DeviceStep> {
        loop {
            if self.shared.stopped() {
                self.cancel();
                return Ok(DeviceStep::FeederEmpty);
            }
            match self.state {
                4 => {
                    if self.shared.paused.load(Ordering::Relaxed) {
                        self.next = Side::Front;
                        return Ok(DeviceStep::FeederEmpty);
                    }
                    if !self.feeder_loaded() {
                        self.next = Side::Front;
                        return Ok(DeviceStep::FeederEmpty);
                    }
                    self.enable()?;
                    self.awaiting_clear = false;
                    self.stop_feeder_sent = false;
                    self.pause_polls = 0;
                    self.shared.set_phase(ScanPhase::Scanning);
                    self.shared
                        .update("Scanning".to_owned(), "Feeding paper.".to_owned());
                }
                5 => match self.pump() {
                    Some(MSG_XFERREADY) => self.state = 6,
                    Some(_) => {
                        self.disable();
                        self.next = Side::Front;
                        return Ok(DeviceStep::FeederEmpty);
                    }
                    None => {
                        if let Some(reason) = self.device_fault.take() {
                            self.cancel();
                            self.needs_reload = true;
                            self.awaiting_clear = true;
                            self.scanner_ready = true;
                            return Ok(DeviceStep::Stopped(reason));
                        }
                        if self.shared.paused.load(Ordering::Relaxed) {
                            // Give paper already in motion time to become an image.
                            self.pause_polls += 1;
                            if self.pause_polls >= PAUSE_GRACE_POLLS {
                                trace("decision", "paused; no image is in progress");
                                self.disable();
                                self.next = Side::Front;
                                return Ok(DeviceStep::FeederEmpty);
                            }
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                },
                _ => return self.begin_image(),
            }
        }
    }

    fn read_row(&mut self, row: &mut [u8]) -> io::Result<RowStep> {
        let index = usize::from(self.next == Side::Front && self.duplex);
        while self.block_row >= self.block_rows {
            if self.transfer_done {
                self.publish_preview(index);
                let tags = self.read_image_tags();
                self.end_transfer(tags);
                return Ok(RowStep::End);
            }
            if let Some(step) = self.read_block() {
                self.publish_preview(index);
                return Ok(step);
            }
        }
        if self.bytes_per_row < row.len() {
            self.cancel();
            self.needs_reload = true;
            return Err(io::Error::other("scanner returned a short row"));
        }
        let start = self.block_row * self.bytes_per_row;
        row.copy_from_slice(&self.buffer[start..start + row.len()]);
        self.block_row += 1;
        let channels = self.color.channels();
        {
            let mut ui = self.shared.ui.lock().expect("ui lock");
            ui.last_progress = Some(std::time::Instant::now());
            ui.panes[index].push(row, channels);
        }
        Ok(RowStep::Row)
    }

    fn abort_side(&mut self) -> io::Result<()> {
        self.publish_preview(usize::from(self.next == Side::Front && self.duplex));
        self.state = self.state.max(7);
        self.cancel();
        self.needs_reload = true;
        Ok(())
    }

    fn wait_for_paper(&mut self, timeout_ms: u32) -> io::Result<bool> {
        let _ = self.pump();
        self.device_fault = None;
        thread::sleep(Duration::from_millis(u64::from(timeout_ms)));
        if self.shared.stopped() {
            return Ok(false);
        }
        if self.shared.paused.load(Ordering::Relaxed) {
            self.loaded_polls = 0;
            return Ok(false);
        }
        if self
            .shared
            .continue_requested
            .swap(false, Ordering::Relaxed)
        {
            (self.needs_reload, self.loaded_polls) = (false, 0);
            trace("decision", "Continue pressed; trying the feeder again");
            return Ok(true);
        }
        // Some drivers stay "online" with the cover or tray open, but cannot report the feeder.
        let ready = self.cap_current(CAP_FEEDERLOADED).is_some();
        match fault_signal(self.scanner_ready, ready) {
            FaultSignal::Opened => {
                if let Some(phase) = phase_when_feeder_unavailable(self.awaiting_clear) {
                    trace(
                        "device",
                        "scanner not ready: feeder state unavailable; cover or tray open, or paper stuck",
                    );
                    self.shared.set_phase(phase);
                    self.shared.update(
                        "Scanner not ready".to_owned(),
                        "The cover or tray is open, or paper is stuck. Open the scanner, remove any stuck paper, and close it. Scanning continues automatically."
                            .to_owned(),
                    );
                } else {
                    trace(
                        "device",
                        "feeder state unavailable during a paper fault; keeping the fault status",
                    );
                }
            }
            FaultSignal::Closed => {
                trace("device", "feeder state available again; scanner is closed");
                self.loaded_polls = 0;
                if self.awaiting_clear {
                    self.awaiting_clear = false;
                    self.needs_reload = false;
                    self.shared.attention_cleared.store(true, Ordering::Relaxed);
                } else {
                    self.shared.set_phase(ScanPhase::WaitingForPaper);
                }
            }
            FaultSignal::Unchanged => {}
        }
        self.scanner_ready = ready;
        if !ready {
            return Ok(false);
        }
        let loaded = self.feeder_loaded();
        if self.pickup_failed_clean && !loaded {
            self.pickup_failed_clean = false;
            self.awaiting_clear = false;
            self.needs_reload = false;
            self.no_media_retries = 0;
            self.loaded_polls = 0;
            self.shared.paper_removed.store(true, Ordering::Relaxed);
            trace(
                "decision",
                "paper left the feeder before any image was captured; nothing to fix, returning to waiting",
            );
            return Ok(false);
        }
        if self.needs_reload {
            if !loaded {
                self.needs_reload = false;
                trace(
                    "decision",
                    "paper removed after the problem; waiting for it to be reloaded",
                );
            }
            self.loaded_polls = 0;
            return Ok(false);
        }
        self.loaded_polls = if loaded {
            self.loaded_polls.saturating_add(1)
        } else {
            0
        };
        if self.loaded_polls >= 2 {
            self.loaded_polls = 0;
            trace(
                "decision",
                "paper stayed loaded for two checks; starting the feeder",
            );
            return Ok(true);
        }
        Ok(false)
    }
}

impl Drop for TwainScanner {
    fn drop(&mut self) {
        self.cancel();
        // SAFETY: the source, source manager, window and module were opened by `open`.
        unsafe {
            if self.state == 4 {
                (self.entry)(
                    &mut self.app,
                    std::ptr::null_mut(),
                    DG_CONTROL,
                    DAT_IDENTITY,
                    MSG_CLOSEDS,
                    (&raw mut self.source).cast(),
                );
            }
            let mut window = self.window;
            (self.entry)(
                &mut self.app,
                std::ptr::null_mut(),
                DG_CONTROL,
                DAT_PARENT,
                MSG_CLOSEDSM,
                (&raw mut window).cast(),
            );
            DestroyWindow(self.window);
            FreeLibrary(self.module);
        }
    }
}

fn scan_thread(
    shared: Arc<ScanShared>,
    profile: Profile,
    config_path: Option<PathBuf>,
    session_stamp: String,
) {
    let device = match TwainScanner::open(shared.clone(), profile.scanner.clone(), config_path) {
        Ok(device) => device,
        Err(error) if error.kind() == io::ErrorKind::Interrupted => return,
        Err(error) => {
            shared.set_phase(ScanPhase::Failed);
            shared.show("Scanner unavailable", error.to_string());
            return;
        }
    };
    let mut app = OwnScannerApp::with_stamp(device, move || session_stamp.clone());
    let ui = shared.clone();
    let mut attention: Option<String> = None;
    let mut sheet_log: Vec<(SavedSide, String)> = Vec::new();
    let mut last_pending = String::new();
    let result = app.run_batch(&profile, &mut |event| {
        match event {
            AppEvent::Started { batch } => {
                trace(
                    "batch",
                    format!("started {batch}; saving to {}", profile.directory.display()),
                );
                trace(
                    "batch",
                    format!(
                        "profile {}: {} dpi, {}, duplex={}, max {} mm, active processing {:?}",
                        profile.name,
                        profile.dpi,
                        profile.color.name(),
                        profile.duplex,
                        profile.max_length_mm,
                        profile.active_processing()
                    ),
                );
            }
            AppEvent::Scanning { sheet, side } => {
                attention = None;
                if sheet_log
                    .first()
                    .is_some_and(|(saved, _)| saved.sheet != *sheet)
                {
                    flush_sheet_log(&mut sheet_log, profile.dpi, &last_pending);
                }
                trace("scan", format!("sheet {sheet:04} {side:?}: started"));
                {
                    let mut state = ui.ui.lock().expect("ui lock");
                    state.processing = Some(*sheet);
                    state.last_progress = Some(std::time::Instant::now());
                }
                ui.set_phase(ScanPhase::Scanning);
                let action = if ui.paused.load(Ordering::Relaxed) {
                    "Pausing: saving sheet"
                } else {
                    "Scanning sheet"
                };
                ui.update(format!("{action} {sheet}"), format!("{side:?}"));
            }
            AppEvent::Saved(side) => {
                let tags = {
                    let mut state = ui.ui.lock().expect("ui lock");
                    state.saved += 1;
                    state.last_progress = Some(std::time::Instant::now());
                    if side.side == Side::Back || !profile.duplex {
                        state.processing = None;
                    }
                    if side.side == Side::Front && state.session.selected.is_none() {
                        state.zoom.pan_x = 0;
                        state.zoom.pan_y = 0;
                    }
                    state.version += 1;
                    last_pending = state.driver_pending.clone();
                    let tags = std::mem::take(&mut state.driver_tags);
                    let UiState { panes, session, .. } = &mut *state;
                    session.add(
                        side.sheet,
                        side.side,
                        &side.file,
                        &panes[usize::from(side.side == Side::Back)],
                    );
                    tags
                };
                ui.set_phase(ScanPhase::Scanned);
                ui.update(
                    format!("Scanned sheet {}", side.sheet),
                    format!("{:?} saved", side.side),
                );
                sheet_log.push((side.clone(), tags));
                if side.side == Side::Back {
                    flush_sheet_log(&mut sheet_log, profile.dpi, &last_pending);
                }
            }
            AppEvent::NeedsAttention { reason, message } => {
                flush_sheet_log(&mut sheet_log, profile.dpi, &last_pending);
                ui.ui.lock().expect("ui lock").processing = None;
                let text = format!("{message} It continues when paper is reloaded.");
                attention = Some(text.clone());
                ui.set_phase(if *reason == StopReason::Jam {
                    ScanPhase::Jam
                } else {
                    ScanPhase::Attention
                });
                ui.show(format!("Stopped: {}", reason.name()), text);
            }
            AppEvent::WaitingForPaper => {
                flush_sheet_log(&mut sheet_log, profile.dpi, &last_pending);
                {
                    let mut state = ui.ui.lock().expect("ui lock");
                    state.processing = None;
                    state.driver_pending.clear();
                }
                if ui.paper_removed.swap(false, Ordering::Relaxed) {
                    attention = None;
                }
                let cover_open = ui.ui.lock().expect("ui lock").phase == ScanPhase::NotReady;
                if cover_open && !ui.attention_cleared.load(Ordering::Relaxed) {
                    // Keep the not-ready message visible until the scanner reports ready.
                } else if ui.paused.load(Ordering::Relaxed) && attention.is_none() {
                    ui.set_phase(ScanPhase::Paused);
                    ui.show_once(
                        "Paused",
                        "Feeding is stopped. Handle the documents, then press Space to resume.",
                    );
                } else if ui.attention_cleared.swap(false, Ordering::Relaxed) {
                    attention = None;
                    ui.set_phase(ScanPhase::WaitingForPaper);
                    ui.show("Jam cleared", "The scanner is closed. Feed documents.");
                } else if attention.is_none() {
                    let waiting = {
                        let state = ui.ui.lock().expect("ui lock");
                        state.status != "Ready" && state.status != "Jam cleared"
                    };
                    if waiting {
                        ui.set_phase(ScanPhase::WaitingForPaper);
                        ui.show("Ready", "Feed documents. Scanning starts automatically.");
                    }
                }
            }
            AppEvent::Finished => {
                flush_sheet_log(&mut sheet_log, profile.dpi, &last_pending);
                trace("batch", "finished; manifest closed");
            }
        }
        if ui.stopped() {
            Flow::Stop
        } else {
            Flow::Continue
        }
    });
    if let Err(error) = result {
        shared.set_phase(ScanPhase::Failed);
        shared.show("Scan failed", error.to_string());
    }
}

static WINDOW_SHARED: OnceLock<Arc<ScanShared>> = OnceLock::new();

unsafe extern "system" fn window_proc(window: Handle, message: u32, w: usize, l: isize) -> isize {
    const WM_DESTROY: u32 = 0x0002;
    const WM_CLOSE: u32 = 0x0010;
    const WM_PAINT: u32 = 0x000f;
    const WM_ERASEBKGND: u32 = 0x0014;

    const WM_TIMER: u32 = 0x0113;
    const WM_SIZE: u32 = 0x0005;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_MOUSEWHEEL: u32 = 0x020a;
    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_LBUTTONDBLCLK: u32 = 0x0203;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_CTLCOLORSTATIC: u32 = 0x0138;
    static LAST_VERSION: Mutex<u64> = Mutex::new(u64::MAX);
    let Some(shared) = WINDOW_SHARED.get() else {
        // SAFETY: forwards the original window message.
        return unsafe { DefWindowProcW(window, message, w, l) };
    };
    // SAFETY: Windows supplies valid handles for this message.
    unsafe {
        match message {
            WM_LBUTTONDOWN => {
                let x = i32::from((l & 0xffff) as i16);
                let y = i32::from(((l >> 16) & 0xffff) as i16);
                let mut client = WinRect::default();
                GetClientRect(window, &mut client);
                if !handle_zoom_click(shared, x, y) && !begin_image_drag(shared, window, x, y) {
                    handle_click(shared, x, y, client.bottom);
                }
                0
            }
            WM_LBUTTONDBLCLK => {
                let x = i32::from((l & 0xffff) as i16);
                let y = i32::from(((l >> 16) & 0xffff) as i16);
                let mut ui = shared.ui.lock().expect("ui lock");
                let area = ui.pane_area;
                if rect_contains(&area, x, y) {
                    let pane_width = ((area.right - area.left - 10) / 2).max(1);
                    let local_x = (x - area.left) % (pane_width + 10);
                    ui.zoom
                        .toggle_inspect(local_x.min(pane_width), y - area.top);
                    ui.version += 1;
                }
                0
            }
            WM_MOUSEMOVE => {
                let mut ui = shared.ui.lock().expect("ui lock");
                let (hover_x, hover_y) = (
                    i32::from((l & 0xffff) as i16),
                    i32::from(((l >> 16) & 0xffff) as i16),
                );
                let toolbar_right = ui.toolbar_right;
                let hover = ZOOM_BUTTONS
                    .iter()
                    .enumerate()
                    .find(|(index, _)| {
                        rect_contains(&zoom_button_rect(*index, toolbar_right), hover_x, hover_y)
                    })
                    .map(|(_, button)| button.0);
                if ui.hover_label != hover {
                    ui.hover_label = hover;
                    ui.version += 1;
                }
                if let Some((start_x, start_y, pan_x, pan_y)) = ui.drag {
                    let x = i32::from((l & 0xffff) as i16);
                    let y = i32::from(((l >> 16) & 0xffff) as i16);
                    ui.zoom.pan_x = (pan_x - (x - start_x)).max(0);
                    ui.zoom.pan_y = (pan_y - (y - start_y)).max(0);
                    ui.version += 1;
                    drop(ui);
                    InvalidateRect(window, std::ptr::null(), 0);
                }
                0
            }
            WM_LBUTTONUP => {
                if shared.ui.lock().expect("ui lock").drag.take().is_some() {
                    ReleaseCapture();
                }
                0
            }
            WM_MOUSEWHEEL => {
                const MK_SHIFT: usize = 0x0004;
                const MK_CONTROL: usize = 0x0008;
                let delta = i32::from(((w >> 16) & 0xffff) as u16 as i16);
                let mut point = WinPoint {
                    x: i32::from((l & 0xffff) as i16),
                    y: i32::from(((l >> 16) & 0xffff) as i16),
                };
                ScreenToClient(window, &mut point);
                let mut ui = shared.ui.lock().expect("ui lock");
                if rect_contains(&ui.pane_area, point.x, point.y) {
                    if w & MK_CONTROL != 0 {
                        let action = if delta > 0 {
                            ZoomAction::In
                        } else {
                            ZoomAction::Out
                        };
                        let area = ui.pane_area;
                        let pane_width = ((area.right - area.left - 10) / 2).max(1);
                        let local_x = (point.x - area.left) % (pane_width + 10);
                        ui.zoom
                            .zoom_at(action, local_x.min(pane_width), point.y - area.top);
                    } else if w & MK_SHIFT != 0 {
                        ui.zoom.pan_x = (ui.zoom.pan_x - delta / 2).max(0);
                    } else {
                        ui.zoom.pan_y = (ui.zoom.pan_y - delta / 2).max(0);
                    }
                } else {
                    ui.session.pinned = true;
                    ui.session.scroll -= delta * UI_SHEET_HEIGHT / 240;
                }
                ui.version += 1;
                0
            }
            WM_TIMER => {
                drain_log_view();
                request_full_images(shared);
                let (mut close_now, mut close_cancelled) = (false, false);
                let version = {
                    let mut ui = shared.ui.lock().expect("ui lock");
                    if ui.close_notice.is_some() && close_cancelled_by(ui.phase) {
                        ui.close_notice = None;
                        ui.version += 1;
                        close_cancelled = true;
                        if std::mem::take(&mut ui.close_set_pause) {
                            // Only undo the pause that Close added, so a reload can continue.
                            shared.paused.store(false, Ordering::Relaxed);
                        }
                    } else if ui.close_notice.is_some() {
                        let current = close_wait_reason(&ui);
                        close_now = current.is_none();
                        if !close_now && ui.close_notice != current {
                            ui.close_notice = current;
                            ui.version += 1;
                        }
                    }
                    ui.version
                };
                if close_cancelled {
                    trace("decision", "close cancelled: a problem needs attention");
                }
                if close_now {
                    trace("decision", "saving finished; closing as requested");
                    DestroyWindow(window);
                    return 0;
                }
                let mut last = LAST_VERSION.lock().expect("version lock");
                if *last != version {
                    *last = version;
                    InvalidateRect(window, std::ptr::null(), 0);
                }
                0
            }
            WM_SIZE => {
                let (width, height) = ((l & 0xffff) as i32, ((l >> 16) & 0xffff) as i32);
                layout_log(width, height);
                InvalidateRect(window, std::ptr::null(), 0);
                0
            }
            WM_CTLCOLORSTATIC if l == LOG_CONTROL.load(Ordering::Relaxed) => {
                static LOG_BRUSH: AtomicIsize = AtomicIsize::new(0);
                let mut brush = LOG_BRUSH.load(Ordering::Relaxed);
                if brush == 0 {
                    brush = CreateSolidBrush(UI_CREAM) as isize;
                    LOG_BRUSH.store(brush, Ordering::Relaxed);
                }
                SetTextColor(w as Handle, UI_TEXT);
                SetBkColor(w as Handle, UI_CREAM);
                brush
            }
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint_window(window, shared);
                0
            }
            WM_CLOSE => {
                let (waiting, first_request) = {
                    let mut ui = shared.ui.lock().expect("ui lock");
                    let reason = close_wait_reason(&ui);
                    let first_request = ui.close_notice.is_none();
                    if reason.is_some() {
                        if first_request {
                            ui.close_set_pause = !shared.paused.load(Ordering::Relaxed);
                        }
                        ui.close_notice = reason.clone();
                        ui.version += 1;
                    }
                    (reason, first_request)
                };
                match waiting {
                    Some(text) => {
                        // Stop taking new paper; everything already scanned is still saved.
                        shared.paused.store(true, Ordering::Relaxed);
                        if first_request {
                            trace("decision", format!("close requested: {text}"));
                        }
                    }
                    None => {
                        DestroyWindow(window);
                    }
                }
                0
            }
            WM_DESTROY => {
                shared.stop.store(true, Ordering::Relaxed);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(window, message, w, l),
        }
    }
}

const PAUSE_RECT: WinRect = WinRect {
    left: 340,
    top: 10,
    right: 376,
    bottom: 44,
};
const RETRY_RECT: WinRect = WinRect {
    left: 384,
    top: 10,
    right: 504,
    bottom: 44,
};

/// Complete sheets are exact; the driver only says whether more images are waiting.
fn progress_text(
    sheets: &[u32],
    images: u32,
    processing: Option<u32>,
    pending: &str,
) -> (String, String) {
    let in_progress = processing.is_some_and(|sheet| sheets.contains(&sheet));
    let complete = sheets.len() - usize::from(in_progress);
    let plural =
        |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    let saved = format!(
        "Saved {} ({})",
        plural(complete, "sheet"),
        plural(images as usize, "image")
    );
    let more = pending == "more";
    let activity = match (processing, more) {
        (Some(sheet), true) => format!("Processing sheet {sheet} · more"),
        (Some(sheet), false) => format!("Processing sheet {sheet}"),
        (None, true) => "More in scanner".to_owned(),
        (None, false) => String::new(),
    };
    (saved, activity)
}

const CLOSE_STALL_LIMIT: Duration = Duration::from_secs(60);

/// Explains why closing must wait; a driver silent past the stall limit cannot trap the user.
fn close_blocked_reason(
    phase: ScanPhase,
    processing: Option<u32>,
    pending: &str,
    stalled: bool,
) -> Option<String> {
    let images_waiting = !pending.is_empty() && pending != "0";
    let active = matches!(phase, ScanPhase::Scanning | ScanPhase::Scanned);
    if stalled || !(active || processing.is_some() || images_waiting) {
        return None;
    }
    let work = processing.map_or("Images are still arriving".to_owned(), |sheet| {
        format!("Sheet {sheet} is still being saved")
    });
    Some(format!(
        "Closing after saving: {work}. Feeding stops as soon as the scanner allows."
    ))
}

/// A problem during the final save must stay on screen instead of closing the window.
fn close_cancelled_by(phase: ScanPhase) -> bool {
    matches!(
        phase,
        ScanPhase::Attention | ScanPhase::Jam | ScanPhase::NotReady | ScanPhase::Failed
    )
}

fn close_wait_reason(ui: &UiState) -> Option<String> {
    let stalled = ui
        .last_progress
        .is_some_and(|moment| moment.elapsed() >= CLOSE_STALL_LIMIT);
    close_blocked_reason(ui.phase, ui.processing, &ui.driver_pending, stalled)
}

fn retry_visible(phase: ScanPhase) -> bool {
    matches!(phase, ScanPhase::Attention | ScanPhase::Jam)
}

fn rect_contains(rect: &WinRect, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

fn sheet_delete_rect(row_y: i32) -> WinRect {
    WinRect {
        left: UI_STRIP_WIDTH - 32,
        top: row_y + 3,
        right: UI_STRIP_WIDTH - 10,
        bottom: row_y + 25,
    }
}

fn is_space_key(message: u32, key: usize) -> bool {
    const WM_KEYDOWN: u32 = 0x0100;
    const VK_SPACE: usize = 0x20;
    message == WM_KEYDOWN && key == VK_SPACE
}

/// Holding a key sends repeats; only the first press may toggle Pause.
fn is_first_key_press(key_flags: isize) -> bool {
    (key_flags >> 30) & 1 == 0
}

fn toggle_pause(shared: &ScanShared) {
    if shared.ui.lock().expect("ui lock").close_notice.is_some() {
        return; // A requested close must not be undone by resuming the feeder.
    }
    let paused = !shared.paused.load(Ordering::Relaxed);
    shared.paused.store(paused, Ordering::Relaxed);
    trace(
        "decision",
        if paused {
            "Pause pressed"
        } else {
            "Resume pressed"
        },
    );
    let mut ui = shared.ui.lock().expect("ui lock");
    if paused && ui.phase == ScanPhase::Scanning {
        ui.status = "Pausing".to_owned();
        ui.detail = "Finishing images already in progress.".to_owned();
    }
    ui.version += 1;
}

fn handle_zoom_click(shared: &ScanShared, x: i32, y: i32) -> bool {
    let toolbar_right = shared.ui.lock().expect("ui lock").toolbar_right;
    let Some((_, action)) = ZOOM_BUTTONS
        .iter()
        .enumerate()
        .find(|(index, _)| rect_contains(&zoom_button_rect(*index, toolbar_right), x, y))
        .map(|(_, button)| *button)
    else {
        return false;
    };
    let mut ui = shared.ui.lock().expect("ui lock");
    if action == ZoomAction::Deskew {
        ui.deskew = !ui.deskew;
    } else {
        if matches!(action, ZoomAction::In | ZoomAction::Out) {
            let area = ui.pane_area;
            let pane_width = ((area.right - area.left - 10) / 2).max(1);
            ui.zoom
                .zoom_at(action, pane_width / 2, (area.bottom - area.top) / 2);
        } else {
            ui.zoom.apply(action);
            if matches!(
                action,
                ZoomAction::Full | ZoomAction::Width | ZoomAction::Height
            ) {
                save_fit_mode(ui.zoom.mode);
            }
        }
    }
    ui.version += 1;
    true
}

fn begin_image_drag(shared: &ScanShared, window: Handle, x: i32, y: i32) -> bool {
    let mut ui = shared.ui.lock().expect("ui lock");
    if !rect_contains(&ui.pane_area, x, y) {
        return false;
    }
    ui.drag = Some((x, y, ui.zoom.pan_x, ui.zoom.pan_y));
    // SAFETY: `window` is the live window receiving this mouse message.
    unsafe {
        SetCapture(window);
        SetCursor(LoadCursorW(std::ptr::null_mut(), 32649 as *const u16));
    }
    true
}

fn handle_click(shared: &ScanShared, x: i32, y: i32, client_bottom: i32) {
    if rect_contains(&PAUSE_RECT, x, y) {
        toggle_pause(shared);
        return;
    }
    let mut ui = shared.ui.lock().expect("ui lock");
    if retry_visible(ui.phase) && rect_contains(&RETRY_RECT, x, y) {
        drop(ui);
        shared.continue_requested.store(true, Ordering::Relaxed);
        trace("decision", "Retry feeding pressed");
        return;
    }
    if x >= UI_STRIP_WIDTH || y < UI_TOP || y >= client_bottom - UI_LOG_HEIGHT {
        return;
    }
    let row = (y - UI_TOP + ui.session.scroll) / UI_SHEET_HEIGHT;
    let Some(sheet) = ui.session.sheets().get(row as usize).copied() else {
        return;
    };
    let row_y = UI_TOP + row * UI_SHEET_HEIGHT - ui.session.scroll;
    if ui.session.selected == Some(sheet)
        && ui.phase != ScanPhase::Scanning
        && rect_contains(&sheet_delete_rect(row_y), x, y)
    {
        drop(ui);
        delete_sheet_now(shared, sheet);
        return;
    }
    ui.session.toggle_sheet(sheet);
    ui.zoom.pan_x = 0;
    ui.zoom.pan_y = 0;
    ui.version += 1;
}

fn delete_sheet_now(shared: &ScanShared, sheet: u32) {
    let result = {
        let mut ui = shared.ui.lock().expect("ui lock");
        if ui.phase == ScanPhase::Scanning {
            return;
        }
        let latest = ui.session.sheets().last().copied();
        let directory = ui.output_dir.clone();
        let result = ui.session.delete_sheet(sheet, &directory);
        if let Ok(moved) = &result {
            ui.saved = ui.saved.saturating_sub(moved.len() as u32);
            if latest == Some(sheet) {
                for pane in &mut ui.panes {
                    pane.shown_pixels.clear();
                    pane.shown_width = 0;
                    pane.shown_rows = 0;
                }
            }
            ui.version += 1;
        }
        result
    };
    match result {
        Ok(moved) => trace(
            "delete",
            format!("sheet {sheet:04}: moved to .deleted: {}", moved.join(", ")),
        ),
        Err(error) => shared.show("Delete failed", error.to_string()),
    }
}

/// SAFETY: `hdc` must be a valid device context.
unsafe fn draw_pause_icon(hdc: Handle, rect: &WinRect, paused: bool) {
    // SAFETY: GDI objects are selected, restored, and deleted within this call.
    unsafe {
        let background = CreateSolidBrush(UI_SAND);
        FillRect(hdc, rect, background);
        DeleteObject(background);
        let ink = CreateSolidBrush(UI_TEXT);
        let (cx, cy) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        if paused {
            let points = [
                WinPoint {
                    x: cx - 6,
                    y: cy - 9,
                },
                WinPoint { x: cx + 9, y: cy },
                WinPoint {
                    x: cx - 6,
                    y: cy + 9,
                },
            ];
            let old_brush = SelectObject(hdc, ink);
            let old_pen = SelectObject(hdc, GetStockObject(8));
            Polygon(hdc, points.as_ptr(), points.len() as i32);
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
        } else {
            for left in [cx - 7, cx + 2] {
                let bar = WinRect {
                    left,
                    top: cy - 9,
                    right: left + 5,
                    bottom: cy + 9,
                };
                FillRect(hdc, &bar, ink);
            }
        }
        DeleteObject(ink);
    }
}

/// SAFETY: `hdc` must be a valid device context.
unsafe fn draw_zoom_icon(hdc: Handle, rect: &WinRect, action: ZoomAction) {
    // SAFETY: the pen is selected, restored, and deleted within this call.
    unsafe {
        let pen = CreatePen(0, 2, UI_TEXT);
        let old_pen = SelectObject(hdc, pen);
        let (cx, cy) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        let line = |x1: i32, y1: i32, x2: i32, y2: i32| {
            MoveToEx(hdc, x1, y1, std::ptr::null_mut());
            LineTo(hdc, x2, y2);
        };
        match action {
            ZoomAction::Full => {
                for (sx, sy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                    let (x, y) = (cx + sx * 9, cy + sy * 9);
                    line(x, y, x - sx * 6, y);
                    line(x, y, x, y - sy * 6);
                }
            }
            ZoomAction::Width => {
                line(cx - 11, cy, cx + 11, cy);
                for side in [-1, 1] {
                    line(cx + side * 11, cy, cx + side * 6, cy - 5);
                    line(cx + side * 11, cy, cx + side * 6, cy + 5);
                }
            }
            ZoomAction::Height => {
                line(cx, cy - 11, cx, cy + 11);
                for side in [-1, 1] {
                    line(cx, cy + side * 11, cx - 5, cy + side * 6);
                    line(cx, cy + side * 11, cx + 5, cy + side * 6);
                }
            }
            ZoomAction::Normal => {
                let text = "1:1".encode_utf16().collect::<Vec<_>>();
                TextOutW(hdc, cx - 11, cy - 8, text.as_ptr(), text.len() as i32);
            }
            ZoomAction::Out => line(cx - 8, cy, cx + 8, cy),
            ZoomAction::In => {
                line(cx - 8, cy, cx + 8, cy);
                line(cx, cy - 8, cx, cy + 8);
            }
            ZoomAction::Deskew => {
                let corners = [
                    (cx - 3, cy - 11),
                    (cx + 10, cy - 5),
                    (cx + 3, cy + 11),
                    (cx - 10, cy + 5),
                ];
                for index in 0..4 {
                    let (a, b) = (corners[index], corners[(index + 1) % 4]);
                    line(a.0, a.1, b.0, b.1);
                }
            }
        }
        SelectObject(hdc, old_pen);
        DeleteObject(pen);
    }
}

/// SAFETY: `hdc` must be a valid device context.
unsafe fn draw_close_icon(hdc: Handle, rect: &WinRect) {
    // SAFETY: the pen is selected, restored, and deleted within this call.
    unsafe {
        let pen = CreatePen(0, 2, UI_TEXT);
        let old_pen = SelectObject(hdc, pen);
        let (left, top, right, bottom) =
            (rect.left + 6, rect.top + 6, rect.right - 6, rect.bottom - 6);
        MoveToEx(hdc, left, top, std::ptr::null_mut());
        LineTo(hdc, right, bottom);
        MoveToEx(hdc, right, top, std::ptr::null_mut());
        LineTo(hdc, left, bottom);
        SelectObject(hdc, old_pen);
        DeleteObject(pen);
    }
}

fn layout_log(width: i32, height: i32) {
    let control = LOG_CONTROL.load(Ordering::Relaxed) as Handle;
    if !control.is_null() {
        // SAFETY: the log control is a live child created on this UI thread.
        unsafe {
            MoveWindow(
                control,
                8,
                height - UI_LOG_HEIGHT + 4,
                (width - 16).max(1),
                UI_LOG_HEIGHT - 10,
                1,
            );
        }
    }
}

fn drain_log_view() {
    const EM_SETSEL: u32 = 0x00b1;
    const EM_SCROLLCARET: u32 = 0x00b7;
    const EM_REPLACESEL: u32 = 0x00c2;
    const MAX_LOG_VIEW: i32 = 7_000_000;
    let control = LOG_CONTROL.load(Ordering::Relaxed) as Handle;
    if control.is_null() {
        return;
    }
    let lines = std::mem::take(&mut *LOG_PENDING.lock().expect("log view lock"));
    if lines.is_empty() {
        return;
    }
    let text = wide(&format!("{}\r\n", lines.join("\r\n")));
    let empty = wide("");
    // SAFETY: the control is live and text buffers outlive these synchronous messages.
    unsafe {
        let mut length = GetWindowTextLengthW(control);
        if length > MAX_LOG_VIEW {
            SendMessageW(control, EM_SETSEL, 0, (length / 2) as isize);
            SendMessageW(control, EM_REPLACESEL, 0, empty.as_ptr() as isize);
            length = GetWindowTextLengthW(control);
        }
        SendMessageW(control, EM_SETSEL, length as usize, length as isize);
        SendMessageW(control, EM_REPLACESEL, 0, text.as_ptr() as isize);
        SendMessageW(control, EM_SCROLLCARET, 0, 0);
    }
}

/// SAFETY: `hdc` must be valid and `pixels` must contain `rows` DWORD-aligned BGR rows.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_bgr_at(
    hdc: Handle,
    pixels: &[u8],
    source_width: usize,
    rows: usize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    let info = BitmapInfoHeader {
        size: std::mem::size_of::<BitmapInfoHeader>() as u32,
        width: source_width as i32,
        height: -(rows as i32),
        planes: 1,
        bit_count: 24,
        compression: 0,
        size_image: 0,
        x_ppm: 0,
        y_ppm: 0,
        colors_used: 0,
        colors_important: 0,
    };
    // SAFETY: the caller guarantees the device context and pixel layout.
    unsafe {
        StretchDIBits(
            hdc,
            x,
            y,
            width,
            height,
            0,
            0,
            source_width as i32,
            rows as i32,
            pixels.as_ptr(),
            &info,
            0,
            0x00cc_0020,
        );
    }
}

/// SAFETY: `hdc` must be valid and `pixels` must contain `rows` DWORD-aligned BGR rows.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_bgr(
    hdc: Handle,
    pixels: &[u8],
    source_width: usize,
    rows: usize,
    x: i32,
    y: i32,
    max_width: i32,
    max_height: i32,
) {
    if source_width == 0 || rows == 0 || max_width <= 0 || max_height <= 0 {
        return;
    }
    let scale =
        (f64::from(max_width) / source_width as f64).min(f64::from(max_height) / rows as f64);
    let info = BitmapInfoHeader {
        size: std::mem::size_of::<BitmapInfoHeader>() as u32,
        width: source_width as i32,
        height: -(rows as i32),
        planes: 1,
        bit_count: 24,
        compression: 0,
        size_image: 0,
        x_ppm: 0,
        y_ppm: 0,
        colors_used: 0,
        colors_important: 0,
    };
    // SAFETY: the caller guarantees the device context and pixel layout.
    unsafe {
        StretchDIBits(
            hdc,
            x,
            y,
            (source_width as f64 * scale).max(1.0) as i32,
            (rows as f64 * scale).max(1.0) as i32,
            0,
            0,
            source_width as i32,
            rows as i32,
            pixels.as_ptr(),
            &info,
            0,
            0x00cc_0020,
        );
    }
}

/// SAFETY: `window` must be a valid window handling WM_PAINT on this thread.
unsafe fn paint_window(window: Handle, shared: &ScanShared) {
    // SAFETY: the paint structure and bitmap buffers remain valid during painting.
    unsafe {
        let mut paint: PaintStruct = std::mem::zeroed();
        let target = BeginPaint(window, &mut paint);
        let mut client = WinRect::default();
        GetClientRect(window, &mut client);
        let (surface_width, surface_height) = (client.right.max(1), client.bottom.max(1));
        let hdc = CreateCompatibleDC(target);
        let bitmap = CreateCompatibleBitmap(target, surface_width, surface_height);
        let previous_bitmap = SelectObject(hdc, bitmap);
        let background = CreateSolidBrush(UI_CREAM);
        FillRect(hdc, &client, background);
        DeleteObject(background);
        SetTextColor(hdc, UI_TEXT);
        SetBkMode(hdc, 1);
        SetStretchBltMode(hdc, 4);
        let mut guard = shared.ui.lock().expect("ui lock");
        let bottom = (client.bottom - UI_LOG_HEIGHT).max(UI_TOP + 40);
        let sheets = guard.session.sheets();
        let max_scroll = (sheets.len() as i32 * UI_SHEET_HEIGHT - (bottom - UI_TOP)).max(0);
        guard.session.scroll = if guard.session.pinned {
            guard.session.scroll.clamp(0, max_scroll)
        } else {
            max_scroll
        };
        if guard.session.selected.is_none() && guard.session.scroll >= max_scroll {
            guard.session.pinned = false;
        }
        guard.toolbar_right = client.right - 8;
        refresh_deskew(&mut guard);
        let ui = &*guard;
        let draw = |x: i32, y: i32, text: &str| {
            let wide = text.encode_utf16().collect::<Vec<_>>();
            TextOutW(hdc, x, y, wide.as_ptr(), wide.len() as i32);
        };
        let (banner, banner_color) = ui.phase.banner();
        let banner_rect = WinRect {
            left: 10,
            top: 10,
            right: 120,
            bottom: 44,
        };
        let banner_brush = CreateSolidBrush(banner_color);
        FillRect(hdc, &banner_rect, banner_brush);
        DeleteObject(banner_brush);
        SetTextColor(hdc, ui.phase.text_color());
        draw(22, 19, banner);
        SetTextColor(hdc, UI_TEXT);
        let (saved_text, activity_text) =
            progress_text(&sheets, ui.saved, ui.processing, &ui.driver_pending);
        draw(132, 8, &saved_text);
        draw(132, 28, &activity_text);
        draw_pause_icon(hdc, &PAUSE_RECT, shared.paused.load(Ordering::Relaxed));
        if retry_visible(ui.phase) {
            let retry_brush = CreateSolidBrush(UI_SAND);
            FillRect(hdc, &RETRY_RECT, retry_brush);
            DeleteObject(retry_brush);
            draw(RETRY_RECT.left + 14, RETRY_RECT.top + 9, "Retry feeding");
        }
        let zoom_panel = zoom_panel_rect(ui.toolbar_right);
        let zoom_panel_brush = CreateSolidBrush(UI_ZOOM_PANEL);
        FillRect(hdc, &zoom_panel, zoom_panel_brush);
        DeleteObject(zoom_panel_brush);
        draw(zoom_panel.left + 10, 19, "Zoom");
        for (index, (_, action)) in ZOOM_BUTTONS.iter().enumerate() {
            let rect = zoom_button_rect(index, ui.toolbar_right);
            let active = ui.zoom.is_active(*action) || (*action == ZoomAction::Deskew && ui.deskew);
            let brush = CreateSolidBrush(if active {
                UI_PEACH
            } else if *action == ZoomAction::Deskew {
                UI_SAND
            } else {
                UI_CREAM
            });
            FillRect(hdc, &rect, brush);
            DeleteObject(brush);
            draw_zoom_icon(hdc, &rect, *action);
        }
        let saved_clip = SaveDC(hdc);
        IntersectClipRect(hdc, 4, UI_TOP, UI_STRIP_WIDTH, bottom);
        for (row, sheet) in sheets.iter().enumerate() {
            let y = UI_TOP + row as i32 * UI_SHEET_HEIGHT - ui.session.scroll;
            if y + UI_SHEET_HEIGHT < UI_TOP || y > bottom {
                continue;
            }
            if ui.session.selected == Some(*sheet) {
                let mark = WinRect {
                    left: 6,
                    top: y,
                    right: UI_STRIP_WIDTH - 4,
                    bottom: y + UI_SHEET_HEIGHT - 2,
                };
                let selected = CreateSolidBrush(UI_SELECTED);
                FillRect(hdc, &mark, selected);
                DeleteObject(selected);
            }
            draw(12, y + 3, &format!("Sheet {sheet}"));
            if ui.session.selected == Some(*sheet) && ui.phase != ScanPhase::Scanning {
                draw_close_icon(hdc, &sheet_delete_rect(y));
            }
            for image in ui
                .session
                .images
                .iter()
                .filter(|image| image.sheet == *sheet)
            {
                let x = if image.side == Side::Front { 12 } else { 104 };
                draw_bgr(
                    hdc,
                    &image.thumb.pixels,
                    image.thumb.width,
                    image.thumb.rows,
                    x,
                    y + 24,
                    THUMB_WIDTH as i32,
                    UI_SHEET_HEIGHT - 30,
                );
            }
        }
        RestoreDC(hdc, saved_clip);
        let left_edge = UI_STRIP_WIDTH + 8;
        let pane_width = ((client.right - left_edge - 18) / 2).max(1);
        const DT_WRAPPED_TEXT: u32 = 0x0010 | 0x0800;
        const DT_CALCRECT: u32 = 0x0400;
        let message = ui
            .close_notice
            .as_ref()
            .map_or_else(|| format!("{}: {}", ui.status, ui.detail), Clone::clone)
            .encode_utf16()
            .collect::<Vec<_>>();
        let message_left = if retry_visible(ui.phase) {
            RETRY_RECT.right + 8
        } else {
            PAUSE_RECT.right + 8
        };
        let message_right = zoom_button_rect(6, ui.toolbar_right).left - 10;
        let mut measured = WinRect {
            left: message_left + 10,
            top: 0,
            right: message_right - 10,
            bottom: 40,
        };
        let has_room = measured.right - measured.left >= 120;
        if has_room {
            DrawTextW(
                hdc,
                message.as_ptr(),
                message.len() as i32,
                &mut measured,
                DT_WRAPPED_TEXT | DT_CALCRECT,
            );
        }
        let content_top = if has_room && measured.bottom <= 34 {
            let pill = WinRect {
                left: message_left,
                top: 10,
                right: message_right,
                bottom: 44,
            };
            let pill_brush = CreateSolidBrush(banner_color);
            FillRect(hdc, &pill, pill_brush);
            DeleteObject(pill_brush);
            let text_top = pill.top + (34 - measured.bottom) / 2;
            let mut text_rect = WinRect {
                left: message_left + 10,
                top: text_top,
                right: message_right - 10,
                bottom: text_top + measured.bottom,
            };
            SetTextColor(hdc, ui.phase.text_color());
            DrawTextW(
                hdc,
                message.as_ptr(),
                message.len() as i32,
                &mut text_rect,
                DT_WRAPPED_TEXT,
            );
            SetTextColor(hdc, UI_TEXT);
            UI_TOP
        } else {
            // A message too long for the toolbar stays fully readable above the images.
            let strip_right = (client.right - 8).max(left_edge + 120);
            let mut text_rect = WinRect {
                left: left_edge + 8,
                top: UI_TOP + 4,
                right: strip_right - 8,
                bottom: UI_TOP + 24,
            };
            DrawTextW(
                hdc,
                message.as_ptr(),
                message.len() as i32,
                &mut text_rect,
                DT_WRAPPED_TEXT | DT_CALCRECT,
            );
            let strip = WinRect {
                left: left_edge,
                top: UI_TOP,
                right: strip_right,
                bottom: text_rect.bottom + 4,
            };
            let strip_brush = CreateSolidBrush(banner_color);
            FillRect(hdc, &strip, strip_brush);
            DeleteObject(strip_brush);
            SetTextColor(hdc, ui.phase.text_color());
            DrawTextW(
                hdc,
                message.as_ptr(),
                message.len() as i32,
                &mut text_rect,
                DT_WRAPPED_TEXT,
            );
            SetTextColor(hdc, UI_TEXT);
            strip.bottom + 4
        };
        let pane_height = (bottom - content_top - 26).max(1);
        let mode = ui
            .session
            .selected
            .map_or("live".to_owned(), |sheet| format!("sheet {sheet}"));
        let image_top = content_top + 22;
        let (mut max_pan_x, mut max_pan_y) = (0, 0);
        let mut shown_scale = None;
        for (index, name) in ["Front", "Back"].into_iter().enumerate() {
            let left = left_edge + index as i32 * (pane_width + 10);
            let note = if ui.deskew {
                ui.deskew_views[index]
                    .as_ref()
                    .map_or(String::new(), |view| {
                        view.angle.map_or(", no angle found".to_owned(), |angle| {
                            format!(", deskew {angle:+.1} deg")
                        })
                    })
            } else {
                String::new()
            };
            draw(left, content_top, &format!("{name} ({mode}{note})"));
            let Some(image) = displayed_image(ui, index) else {
                continue;
            };
            let scale = ui.zoom.scale_for(
                image.source_width,
                image.source_rows,
                pane_width,
                pane_height,
            );
            shown_scale.get_or_insert(scale);
            let width = ((image.source_width as f64 * scale).round() as i32).max(1);
            let height = ((image.source_rows as f64 * scale).round() as i32).max(1);
            let (limit_x, limit_y) = ((width - pane_width).max(0), (height - pane_height).max(0));
            max_pan_x = max_pan_x.max(limit_x);
            max_pan_y = max_pan_y.max(limit_y);
            let saved_pane = SaveDC(hdc);
            IntersectClipRect(
                hdc,
                left,
                image_top,
                left + pane_width,
                image_top + pane_height,
            );
            draw_bgr_at(
                hdc,
                image.pixels,
                image.width,
                image.rows,
                left - ui.zoom.pan_x.clamp(0, limit_x),
                image_top - ui.zoom.pan_y.clamp(0, limit_y),
                width,
                height,
            );
            RestoreDC(hdc, saved_pane);
            if limit_x > 0 || limit_y > 0 {
                let scroll_brush = CreateSolidBrush(UI_SCROLL);
                if limit_y > 0 {
                    let (start, length) =
                        scroll_thumb(pane_height, height, ui.zoom.pan_y.clamp(0, limit_y));
                    let thumb = WinRect {
                        left: left + pane_width - 7,
                        top: image_top + start,
                        right: left + pane_width - 2,
                        bottom: image_top + start + length,
                    };
                    FillRect(hdc, &thumb, scroll_brush);
                }
                if limit_x > 0 {
                    let (start, length) =
                        scroll_thumb(pane_width, width, ui.zoom.pan_x.clamp(0, limit_x));
                    let thumb = WinRect {
                        left: left + start,
                        top: image_top + pane_height - 7,
                        right: left + start + length,
                        bottom: image_top + pane_height - 2,
                    };
                    FillRect(hdc, &thumb, scroll_brush);
                }
                DeleteObject(scroll_brush);
            }
        }
        if let Some(scale) = shown_scale {
            draw(zoom_percent_x(ui.toolbar_right), 19, &zoom_percent(scale));
        }
        if let Some(label) = ui.hover_label {
            const DT_SINGLE_LINE: u32 = 0x0020 | 0x0800;
            let text = label.encode_utf16().collect::<Vec<_>>();
            let mut size = WinRect {
                left: 0,
                top: 0,
                right: 500,
                bottom: 20,
            };
            DrawTextW(
                hdc,
                text.as_ptr(),
                text.len() as i32,
                &mut size,
                DT_SINGLE_LINE | 0x0400,
            );
            let right = zoom_panel_rect(ui.toolbar_right).right;
            let tip = WinRect {
                left: right - size.right - 12,
                top: 50,
                right,
                bottom: 72,
            };
            let tip_brush = CreateSolidBrush(UI_SAND);
            FillRect(hdc, &tip, tip_brush);
            DeleteObject(tip_brush);
            let mut text_rect = WinRect {
                left: tip.left + 6,
                top: tip.top + 3,
                right: tip.right - 6,
                bottom: tip.bottom,
            };
            DrawTextW(
                hdc,
                text.as_ptr(),
                text.len() as i32,
                &mut text_rect,
                DT_SINGLE_LINE,
            );
        }
        guard.pane_area = WinRect {
            left: left_edge,
            top: image_top,
            right: left_edge + 2 * pane_width + 10,
            bottom: image_top + pane_height,
        };
        guard.zoom.pan_x = guard.zoom.pan_x.clamp(0, max_pan_x);
        guard.zoom.pan_y = guard.zoom.pan_y.clamp(0, max_pan_y);
        if let Some(scale) = shown_scale {
            guard.zoom.scale = scale;
        }
        drop(guard);
        BitBlt(
            target,
            0,
            0,
            surface_width,
            surface_height,
            hdc,
            0,
            0,
            0x00cc_0020,
        );
        SelectObject(hdc, previous_bitmap);
        DeleteObject(bitmap);
        DeleteDC(hdc);
        EndPaint(window, &paint);
    }
}

fn run_window(
    profile: Profile,
    config_source: &str,
    config_path: Option<PathBuf>,
) -> io::Result<i32> {
    const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_CHILD: u32 = 0x4000_0000;
    const CW_USEDEFAULT: i32 = i32::MIN;
    let session_stamp = local_stamp();
    open_log(&profile.directory, &profile.prefix, &session_stamp)?;
    trace("config", format!("using {config_source}"));
    if cfg!(debug_assertions) {
        trace(
            "warning",
            "unoptimized build: PNG encoding is much slower. Rebuild with rustc -O.",
        );
    }
    let shared = Arc::new(ScanShared::default());
    {
        let fit = load_fit_mode();
        let mut ui = shared.ui.lock().expect("ui lock");
        ui.output_dir = profile.directory.clone();
        ui.zoom.mode = fit;
        ui.zoom.fit = fit;
    }
    shared.show("Starting", "Opening the scanner.");
    WINDOW_SHARED
        .set(shared.clone())
        .map_err(|_| io::Error::other("window already started"))?;
    let class_name = wide("OwnScannerWindow");

    // SAFETY: class and window strings live through creation; messages run on this thread.
    let window = unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = WndClassW {
            style: 0x0008,
            proc_: Some(window_proc),
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: std::ptr::null_mut(),
            cursor: LoadCursorW(std::ptr::null_mut(), 32512 as *const u16),
            background: std::ptr::null_mut(),
            menu_name: std::ptr::null(),
            class_name: class_name.as_ptr(),
        };
        if RegisterClassW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            wide(&format!(
                "OwnScanner {VERSION}{}",
                if cfg!(debug_assertions) {
                    " - SLOW UNOPTIMIZED BUILD"
                } else {
                    ""
                }
            ))
            .as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE | 0x0200_0000, // WS_CLIPCHILDREN keeps buttons stable.
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            850,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        );
        if window.is_null() {
            return Err(io::Error::last_os_error());
        }
        const LOG_STYLE: u32 =
            WS_CHILD | WS_VISIBLE | 0x0020_0000 | 0x0080_0000 | 0x4 | 0x40 | 0x800;
        let log = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide("").as_ptr(),
            LOG_STYLE,
            0,
            0,
            100,
            100,
            window,
            10usize as Handle,
            instance,
            std::ptr::null_mut(),
        );
        if !log.is_null() {
            SendMessageW(log, 0x00c5, 8_000_000, 0);
            SendMessageW(log, 0x0030, GetStockObject(11) as usize, 1);
            LOG_CONTROL.store(log as isize, Ordering::Relaxed);
            let mut client = WinRect::default();
            GetClientRect(window, &mut client);
            layout_log(client.right, client.bottom);
        }
        SetTimer(window, 1, 100, std::ptr::null_mut());
        window
    };
    let preview_cache =
        preview_cache_dir().join(format!("ownscanner-preview-{}.bin", std::process::id()));
    if let Err(error) = shared
        .ui
        .lock()
        .expect("ui lock")
        .session
        .open_cache(preview_cache)
    {
        trace("session", format!("preview cache unavailable: {error}"));
    }
    let _ = window;
    let worker = {
        let shared = shared.clone();
        thread::spawn(move || scan_thread(shared, profile, config_path, session_stamp))
    };
    // SAFETY: `message` is valid for the standard message loop.
    unsafe {
        let mut message: WinMsg = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            if is_space_key(message.message, message.w_param) {
                if is_first_key_press(message.l_param) {
                    toggle_pause(&shared);
                }
                continue;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    shared.stop.store(true, Ordering::Relaxed);
    worker
        .join()
        .map_err(|_| io::Error::other("scanner thread failed"))?;
    Ok(0)
}

const CONFIG_SCHEMA_VERSION: i64 = 1;

type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

fn home_dir(env: EnvLookup<'_>) -> Option<PathBuf> {
    env("HOME")
        .or_else(|| env("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn xdg_dir(env: EnvLookup<'_>, variable: &str, fallback: &str) -> Option<PathBuf> {
    env(variable)
        .filter(|value| Path::new(value).is_absolute())
        .map(PathBuf::from)
        .or_else(|| home_dir(env).map(|home| home.join(fallback)))
        .map(|base| base.join("ownscanner"))
}

fn global_config_path(env: EnvLookup<'_>) -> Option<PathBuf> {
    xdg_dir(env, "XDG_CONFIG_HOME", ".config").map(|dir| dir.join("config.toml"))
}

/// Resolves forced, local, global, then system configuration; the first file wins.
fn resolve_config(
    forced: Option<&Path>,
    current_dir: &Path,
    env: EnvLookup<'_>,
) -> io::Result<Option<(&'static str, PathBuf)>> {
    if let Some(path) = forced {
        if path.is_file() {
            return Ok(Some(("forced", path.to_path_buf())));
        }
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("config file not found: {}", path.display()),
        ));
    }
    let mut candidates = vec![("local", current_dir.join(".ownscanner.toml"))];
    if let Some(path) = global_config_path(env) {
        candidates.push(("global", path));
    }
    if let Some(root) = env("ProgramData").filter(|value| !value.is_empty()) {
        candidates.push(("system", Path::new(&root).join("ownscanner/config.toml")));
    }
    Ok(candidates.into_iter().find(|(_, path)| path.is_file()))
}

#[derive(Debug, PartialEq)]
enum ConfigValue {
    Text(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
}

fn parse_config_value(raw: &str) -> Option<ConfigValue> {
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        return Some(ConfigValue::Text(raw[1..raw.len() - 1].to_owned()));
    }
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        let inner = &raw[1..raw.len() - 1];
        if inner.replace("\\\\", "").contains('\\') {
            return None;
        }
        return Some(ConfigValue::Text(inner.replace("\\\\", "\\")));
    }
    match raw {
        "true" => Some(ConfigValue::Bool(true)),
        "false" => Some(ConfigValue::Bool(false)),
        _ => raw
            .parse::<i64>()
            .map(ConfigValue::Integer)
            .ok()
            .or_else(|| raw.parse::<f64>().ok().map(ConfigValue::Float)),
    }
}

fn strip_config_comment(line: &str) -> &str {
    let mut quote = None;
    for (index, character) in line.char_indices() {
        match (quote, character) {
            (None, '\'' | '"') => quote = Some(character),
            (Some(open), _) if open == character => quote = None,
            (None, '#') => return &line[..index],
            _ => {}
        }
    }
    line
}

/// Reads the versioned OwnScanner configuration and fails on unknown settings.
fn parse_config(text: &str) -> Result<Profile, String> {
    let mut profile = Profile::long_raw(PathBuf::new());
    let mut section = String::new();
    let mut schema = None;
    for (index, source) in text.lines().enumerate() {
        let number = index + 1;
        let line = strip_config_comment(source).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_owned();
            if section != "scan" {
                return Err(format!("line {number}: unknown section [{section}]"));
            }
            continue;
        }
        let (key, raw) = line
            .split_once('=')
            .ok_or_else(|| format!("line {number}: expected key = value"))?;
        let (key, raw) = (key.trim(), raw.trim());
        let value =
            parse_config_value(raw).ok_or_else(|| format!("line {number}: invalid value {raw}"))?;
        let invalid = || format!("line {number}: invalid value for {key}");
        match (section.as_str(), key, value) {
            ("", "schema_version", ConfigValue::Integer(value)) => schema = Some(value),
            ("scan", "output_dir", ConfigValue::Text(value)) => {
                profile.directory = PathBuf::from(value)
            }
            ("scan", "prefix", ConfigValue::Text(value)) => profile.prefix = value,
            ("scan", "read_ahead", ConfigValue::Bool(value)) => profile.read_ahead = value,
            ("scan", "deskew", ConfigValue::Bool(value)) => profile.deskew = value,
            ("scan", "scanner", ConfigValue::Text(value)) => {
                profile.scanner = (!value.is_empty()).then_some(value)
            }
            ("scan", "dpi", ConfigValue::Integer(value)) => {
                profile.dpi = u32::try_from(value).map_err(|_| invalid())?
            }
            ("scan", "color", ConfigValue::Text(value)) => {
                profile.color = match value.as_str() {
                    "rgb24" => ColorMode::Rgb24,
                    "gray8" => ColorMode::Gray8,
                    _ => return Err(invalid()),
                }
            }
            ("scan", "duplex", ConfigValue::Bool(value)) => profile.duplex = value,
            ("scan", "max_length_mm", ConfigValue::Integer(value)) => {
                profile.max_length_mm = u32::try_from(value).map_err(|_| invalid())?
            }
            ("scan", "margin_mm", ConfigValue::Float(value)) => profile.margin_mm = value,
            ("scan", "margin_mm", ConfigValue::Integer(value)) => profile.margin_mm = value as f64,
            ("", "schema_version", _)
            | (
                "scan",
                "output_dir" | "scanner" | "read_ahead" | "deskew" | "prefix" | "dpi" | "color"
                | "duplex" | "max_length_mm" | "margin_mm",
                _,
            ) => return Err(invalid()),
            _ => return Err(format!("line {number}: unknown setting {key}")),
        }
    }
    match schema {
        Some(CONFIG_SCHEMA_VERSION) => {}
        Some(other) => return Err(format!("unsupported schema_version {other}")),
        None => return Err("missing schema_version".to_owned()),
    }
    profile
        .validate()
        .map_err(|error| format!("invalid settings: {error}"))?;
    Ok(profile)
}

fn load_profile(
    forced: Option<&Path>,
    current_dir: &Path,
    env: EnvLookup<'_>,
) -> io::Result<(Profile, String)> {
    let Some((level, path)) = resolve_config(forced, current_dir, env)? else {
        return Ok((
            Profile::long_raw(PathBuf::new()),
            "built-in defaults".to_owned(),
        ));
    };
    let text = fs::read_to_string(&path)?;
    let profile = parse_config(&text).map_err(|message| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {message}", path.display()),
        )
    })?;
    Ok((profile, format!("{level} {}", path.display())))
}

fn config_template(output_dir: &Path) -> io::Result<String> {
    let folder = output_dir.display().to_string();
    if folder.contains('\'') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "output folder cannot contain a single quote",
        ));
    }
    Ok(format!(
        "# OwnScanner configuration\nschema_version = {CONFIG_SCHEMA_VERSION}\n\n[scan]\noutput_dir = '{folder}'\n# scanner = 'Exact TWAIN source name' # optional; detected automatically\nprefix = \"scan\"\nread_ahead = true # faster; Pause can still save sheets already read\ndeskew = false # true makes the driver rotate saved images\ndpi = 600\ncolor = \"rgb24\" # rgb24 or gray8\nduplex = true\nmax_length_mm = 5588 # safety limit; the driver reports its own limits\nmargin_mm = 0.0 # reserved for A/B test AB-3\n"
    ))
}

fn init_config(path: &Path, output_dir: &Path) -> io::Result<()> {
    let output_dir = std::path::absolute(output_dir)?;
    let text = config_template(&output_dir)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

#[derive(Debug, PartialEq, Eq)]
enum SourceChoice {
    Use { name: String, reason: &'static str },
    Ask { reason: String },
    None,
}

/// Chooses configured, only, then system-default scanner; otherwise asks.
fn choose_source(
    sources: &[String],
    configured: Option<&str>,
    system_default: Option<&str>,
) -> SourceChoice {
    let found = |name: &str| sources.iter().any(|source| source == name);
    if sources.is_empty() {
        return SourceChoice::None;
    }
    if let Some(name) = configured {
        return if found(name) {
            SourceChoice::Use {
                name: name.to_owned(),
                reason: "configured",
            }
        } else {
            SourceChoice::Ask {
                reason: format!("configured scanner `{name}` was not found"),
            }
        };
    }
    if let [only] = sources {
        return SourceChoice::Use {
            name: only.clone(),
            reason: "only scanner found",
        };
    }
    if let Some(name) = system_default.filter(|name| found(name)) {
        return SourceChoice::Use {
            name: name.to_owned(),
            reason: "system default",
        };
    }
    SourceChoice::Ask {
        reason: "several scanners were found and none is the default".to_owned(),
    }
}

fn twain_sources(entry: DsmEntry, app: &mut TwIdentity) -> (Vec<TwIdentity>, Option<String>) {
    let mut sources = Vec::new();
    let mut source = empty_identity();
    let mut message = MSG_GETFIRST;
    // SAFETY: `app` and `source` are valid TW_IDENTITY values for source-manager calls.
    unsafe {
        while entry(
            app,
            std::ptr::null_mut(),
            DG_CONTROL,
            DAT_IDENTITY,
            message,
            (&raw mut source).cast(),
        ) == TWRC_SUCCESS
        {
            sources.push(source);
            message = MSG_GETNEXT;
        }
        let mut default_source = empty_identity();
        let default_name = (entry(
            app,
            std::ptr::null_mut(),
            DG_CONTROL,
            DAT_IDENTITY,
            MSG_GETDEFAULT,
            (&raw mut default_source).cast(),
        ) == TWRC_SUCCESS)
            .then(|| tw_string(&{ default_source.product_name }));
        (sources, default_name)
    }
}

/// Adds or replaces the scanner setting while keeping other config lines.
fn config_with_scanner(text: &str, name: &str) -> Result<String, String> {
    if name.contains('\'') {
        return Err("scanner name cannot contain a single quote".to_owned());
    }
    let setting = format!("scanner = '{name}'");
    let mut lines = Vec::new();
    let (mut in_scan, mut saw_scan, mut replaced) = (false, false, false);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_scan = trimmed == "[scan]";
            saw_scan |= in_scan;
        }
        let key = trimmed.split('=').next().unwrap_or("").trim();
        if in_scan && key == "scanner" {
            if !replaced {
                lines.push(setting.clone());
                replaced = true;
            }
            continue;
        }
        lines.push(line.to_owned());
        if trimmed == "[scan]"
            && !text
                .lines()
                .any(|other| other.trim().split('=').next().unwrap_or("").trim() == "scanner")
        {
            lines.push(setting.clone());
            replaced = true;
        }
    }
    if !saw_scan {
        lines.push("[scan]".to_owned());
        lines.push(setting);
    }
    let mut updated = lines.join("\n");
    updated.push('\n');
    parse_config(&updated)?;
    Ok(updated)
}

fn remember_scanner(path: &Path, name: &str) -> io::Result<()> {
    let updated = config_with_scanner(&fs::read_to_string(path)?, name)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let written = file
        .write_all(updated.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    fs::rename(&temp, path)
}

fn real_env(name: &str) -> Option<String> {
    env::var(name).ok()
}

fn preview_cache_dir() -> PathBuf {
    xdg_dir(&real_env, "XDG_CACHE_HOME", ".cache")
        .filter(|dir| fs::create_dir_all(dir).is_ok())
        .unwrap_or_else(env::temp_dir)
}

fn split_config_flag(args: &[String]) -> Result<(Option<PathBuf>, Vec<&str>), String> {
    let mut forced = None;
    let mut parts = Vec::new();
    let mut items = args.iter();
    while let Some(arg) = items.next() {
        if arg == "--config" {
            let path = items.next().ok_or("--config requires a file")?;
            forced = Some(PathBuf::from(path));
        } else {
            parts.push(arg.as_str());
        }
    }
    Ok((forced, parts))
}

fn run_scan(
    forced: Option<&Path>,
    folder: Option<&str>,
    disable_read_ahead: bool,
    scanner_deskew: bool,
) -> io::Result<i32> {
    let (mut profile, source) = load_profile(forced, &env::current_dir()?, &real_env)?;
    if let Some(folder) = folder {
        profile.directory = PathBuf::from(folder);
    }
    if disable_read_ahead {
        profile.read_ahead = false;
    }
    if scanner_deskew {
        profile.deskew = true;
    }
    if profile.directory.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no output folder; run `ownscanner config init <folder>` or `ownscanner scan <folder>`",
        ));
    }
    let config_path =
        resolve_config(forced, &env::current_dir()?, &real_env)?.map(|(_, path)| path);
    run_window(profile, &source, config_path)
}

fn show_config(forced: Option<&Path>, out: &mut dyn Write) -> io::Result<i32> {
    let (profile, source) = load_profile(forced, &env::current_dir()?, &real_env)?;
    writeln!(
        out,
        "source: {source}\noutput_dir: {}\nprefix: {}\ndpi: {}\ncolor: {}\nduplex: {}\nread_ahead: {}\ndeskew: {}\nmax_length_mm: {}\nmargin_mm: {}",
        profile.directory.display(),
        profile.prefix,
        profile.dpi,
        profile.color.name(),
        profile.duplex,
        profile.read_ahead,
        profile.deskew,
        profile.max_length_mm,
        profile.margin_mm
    )?;
    Ok(0)
}

/// Read-only driver answers used to explain which setting values are possible.
#[derive(Default)]
struct DriverValues {
    sources: Vec<String>,
    selected: Option<String>,
    caps: std::collections::BTreeMap<u16, Vec<String>>,
}

fn parse_driver_values(text: &str) -> DriverValues {
    let mut driver = DriverValues::default();
    for line in text.lines() {
        if let Some(source) = line.strip_prefix("source: ") {
            driver
                .sources
                .push(source.trim_end_matches(" (system default)").to_owned());
        } else if let Some(selected) = line.strip_prefix("selected: ") {
            driver.selected = selected.split(" (").next().map(str::to_owned);
        } else if let (Some(start), Some(end), Some(options)) = (
            line.find("[0x"),
            line.find("] current="),
            line.split_once(" options=").map(|(_, options)| options),
        ) && let Ok(cap) = u16::from_str_radix(&line[start + 3..end], 16)
        {
            let values = options
                .split(',')
                .filter(|value| !value.is_empty())
                .map(|value| value.strip_suffix(".000").unwrap_or(value).to_owned())
                .collect();
            driver.caps.insert(cap, values);
        }
    }
    driver
}

fn config_value_rows(
    profile: &Profile,
    driver: Option<&DriverValues>,
) -> Vec<(&'static str, String, String)> {
    let options = |cap: u16| {
        driver
            .and_then(|driver| driver.caps.get(&cap))
            .filter(|values| !values.is_empty())
            .map(|values| values.join(", "))
    };
    let colors = driver
        .and_then(|driver| driver.caps.get(&ICAP_PIXELTYPE))
        .map_or("rgb24, gray8".to_owned(), |values| {
            let mut names = Vec::new();
            if values.iter().any(|value| value == "2") {
                names.push("rgb24");
            }
            if values.iter().any(|value| value == "1") {
                names.push("gray8");
            }
            names.join(", ")
        });
    let max_length = driver
        .and_then(|driver| driver.caps.get(&0x1112))
        .and_then(|values| values.last())
        .and_then(|inches| inches.parse::<f64>().ok())
        .map_or("positive whole number".to_owned(), |inches| {
            format!("1..{:.0}", inches * 25.4)
        });
    let scanners = driver
        .filter(|driver| !driver.sources.is_empty())
        .map_or("exact TWAIN source name".to_owned(), |driver| {
            driver.sources.join(", ")
        });
    vec![
        (
            "output_dir",
            profile.directory.display().to_string(),
            "any folder".to_owned(),
        ),
        (
            "scanner",
            profile.scanner.clone().unwrap_or("auto".to_owned()),
            scanners,
        ),
        (
            "prefix",
            profile.prefix.clone(),
            "text without / \\ : * ? \" < > |".to_owned(),
        ),
        (
            "read_ahead",
            profile.read_ahead.to_string(),
            options(CAP_AUTOSCAN).unwrap_or("true, false".to_owned()),
        ),
        (
            "deskew",
            profile.deskew.to_string(),
            options(0x1151).unwrap_or("true, false".to_owned()),
        ),
        (
            "dpi",
            profile.dpi.to_string(),
            options(0x1118).unwrap_or("positive whole number".to_owned()),
        ),
        ("color", profile.color.name().to_owned(), colors),
        (
            "duplex",
            profile.duplex.to_string(),
            options(CAP_DUPLEXENABLED).unwrap_or("true, false".to_owned()),
        ),
        (
            "max_length_mm",
            profile.max_length_mm.to_string(),
            max_length,
        ),
        (
            "margin_mm",
            profile.margin_mm.to_string(),
            "reserved, no effect yet".to_owned(),
        ),
    ]
}

fn format_config_values(
    rows: &[(&'static str, String, String)],
    only: Option<&str>,
) -> Result<String, String> {
    let chosen = rows
        .iter()
        .filter(|(key, _, _)| only.is_none_or(|wanted| wanted == *key))
        .collect::<Vec<_>>();
    if chosen.is_empty() {
        let keys = rows
            .iter()
            .map(|(key, _, _)| *key)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "unknown setting `{}`; use one of: {keys}",
            only.unwrap_or("")
        ));
    }
    let key_width = chosen
        .iter()
        .map(|row| row.0.len())
        .max()
        .unwrap_or(0)
        .max(7);
    let value_width = chosen
        .iter()
        .map(|row| row.1.chars().count())
        .max()
        .unwrap_or(0)
        .max(7);
    let mut text = format!(
        "{:<key_width$}  {:<value_width$}  allowed\n",
        "setting", "current"
    );
    for (key, value, allowed) in chosen {
        text.push_str(&format!(
            "{key:<key_width$}  {value:<value_width$}  {allowed}\n"
        ));
    }
    Ok(text)
}

fn show_config_values(
    forced: Option<&Path>,
    only: Option<&str>,
    out: &mut dyn Write,
) -> io::Result<i32> {
    let (profile, source) = load_profile(forced, &env::current_dir()?, &real_env)?;
    let mut probe = Vec::new();
    let driver = match twain_probe(&mut probe, profile.scanner.as_deref(), false) {
        Ok(0) => Some(parse_driver_values(&String::from_utf8_lossy(&probe))),
        _ => None,
    };
    let text = format_config_values(&config_value_rows(&profile, driver.as_ref()), only)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    let device = driver
        .as_ref()
        .and_then(|driver| driver.selected.as_deref())
        .unwrap_or("unavailable, showing generic values");
    write!(out, "config: {source}\ndriver: {device}\n\n{text}")?;
    Ok(0)
}

fn usage() -> String {
    format!(
        "OwnScanner {VERSION}\n\nUsage:\n  ownscanner scan [folder]   Open the scan window; folder overrides config\n  ownscanner config path     Show the selected and global config paths\n  ownscanner config show     Show the effective settings\n  ownscanner config values [setting]  Show current and allowed values, asking the driver\n  ownscanner config init <folder>  Create ~/.config/ownscanner/config.toml\n  --config <file>            Use only this config file\n  --disable-read-ahead       Stop closer to Pause, but scan more slowly\n  --deskew                   Let the driver straighten saved images\n  ownscanner scanners        List detected scanners and the selected default\n  ownscanner probe           Read selected scanner capabilities without feeding paper\n  ownscanner pair <folder>   Pair CapturePerfect scans without changing them\n  ownscanner --version\n  ownscanner --help\n\nDevelopment:\n  --dev-autoformat           Format ownscanner.rs\n  --dev-autoupdate           Rebuild from ownscanner.rs when newer\n  --dev-test                 Compile and run embedded tests"
    )
}

fn run_cli(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (forced, mut parts) = match split_config_flag(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "ownscanner: {message}");
            return 2;
        }
    };
    let forced = forced.as_deref();
    let disable_read_ahead = parts.contains(&"--disable-read-ahead");
    parts.retain(|part| *part != "--disable-read-ahead");
    let scanner_deskew = parts.contains(&"--deskew");
    parts.retain(|part| *part != "--deskew");
    // Help and mistyped options must never reach a command that opens the scanner.
    if parts.iter().any(|part| matches!(*part, "--help" | "-h")) {
        return writeln!(err, "{}", usage()).map_or(1, |()| 0);
    }
    if parts.len() > 1
        && let Some(option) = parts.iter().find(|part| part.starts_with('-'))
    {
        let _ = writeln!(err, "ownscanner: unknown option `{option}`\n\n{}", usage());
        return 2;
    }
    let report = |result: io::Result<i32>, err: &mut dyn Write| match result {
        Ok(code) => Ok(code),
        Err(error) => writeln!(err, "ownscanner: {error}").map(|()| 1),
    };
    let result = match parts.as_slice() {
        ["config", "path"] => {
            let selected =
                env::current_dir().and_then(|current| resolve_config(forced, &current, &real_env));
            match selected {
                Ok(selected) => {
                    let selected = selected
                        .map_or("built-in defaults".to_owned(), |(level, path)| {
                            format!("{level} {}", path.display())
                        });
                    let global = global_config_path(&real_env)
                        .map_or("unavailable".to_owned(), |path| path.display().to_string());
                    writeln!(out, "selected: {selected}\nglobal: {global}").map(|()| 0)
                }
                Err(error) => writeln!(err, "ownscanner: {error}").map(|()| 1),
            }
        }
        ["config", "show"] => {
            let shown = show_config(forced, out);
            report(shown, err)
        }
        ["config", "values"] => {
            let shown = show_config_values(forced, None, out);
            report(shown, err)
        }
        ["config", "values", setting] => {
            let shown = show_config_values(forced, Some(setting), out);
            report(shown, err)
        }
        ["config", "init", folder] => {
            let created = global_config_path(&real_env)
                .ok_or_else(|| io::Error::other("cannot determine the home directory"))
                .and_then(|path| {
                    init_config(&path, Path::new(folder))?;
                    writeln!(out, "created {}", path.display())?;
                    Ok(0)
                });
            report(created, err)
        }
        ["scan"] => {
            let scanned = run_scan(forced, None, disable_read_ahead, scanner_deskew);
            report(scanned, err)
        }
        [] | ["--help"] | ["-h"] => writeln!(err, "{}", usage()).map(|()| 0),
        ["--version"] | ["-V"] => writeln!(out, "ownscanner {VERSION}").map(|()| 0),
        ["scan", directory] => {
            let scanned = run_scan(forced, Some(directory), disable_read_ahead, scanner_deskew);
            report(scanned, err)
        }
        ["probe"] | ["scanners"] => {
            let list_only = parts.as_slice() == ["scanners"];
            let probed = env::current_dir()
                .and_then(|current| load_profile(forced, &current, &real_env))
                .and_then(|(profile, _)| twain_probe(out, profile.scanner.as_deref(), list_only));
            report(probed, err)
        }
        ["pair", directory] => match pair_capture_perfect(Path::new(directory)) {
            Ok(report) => {
                let backs = report
                    .sheets
                    .iter()
                    .filter(|sheet| sheet.back.is_some())
                    .count();
                writeln!(
                    out,
                    "sheets: {}\nwith back: {backs}\nfront only: {}\norphan backs: {}\nignored: {}",
                    report.sheets.len(),
                    report.sheets.len() - backs,
                    report.orphan_backs.len(),
                    report.ignored
                )
                .map(|()| 0)
            }
            Err(error) => writeln!(err, "ownscanner: {error}").map(|()| 1),
        },
        [command, ..] => writeln!(
            err,
            "ownscanner: unknown command `{command}`\n\n{}",
            usage()
        )
        .map(|()| 2),
    };
    result.unwrap_or(1)
}

fn consume_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|arg| arg != flag);
    args.len() != before
}

fn dev_test() -> io::Result<i32> {
    fs::create_dir_all("target")?;
    let test_exe = Path::new("target").join("ownscanner_test.exe");
    let compiled = Command::new("rustc")
        .args(["--edition", "2024", "-O", "--test", "ownscanner.rs", "-o"])
        .arg(&test_exe)
        .status()?;
    if !compiled.success() {
        return Ok(compiled.code().unwrap_or(1));
    }
    Ok(Command::new(&test_exe).status()?.code().unwrap_or(1))
}

fn run() -> io::Result<i32> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if consume_flag(&mut args, "--dev-autoformat") {
        let status = Command::new("rustfmt")
            .args(["--edition", "2024", "ownscanner.rs"])
            .status()?;
        if !status.success() {
            return Ok(status.code().unwrap_or(1));
        }
    }
    if let Some(code) = dev_autoupdate::handle(&mut args, "ownscanner.rs", "ownscanner")? {
        return Ok(code);
    }
    if consume_flag(&mut args, "--dev-test") {
        return dev_test();
    }
    Ok(run_cli(&args, &mut io::stdout(), &mut io::stderr()))
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("ownscanner: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    const STAMP: &str = "20261121161524305";

    #[derive(Clone, Copy)]
    struct Page {
        height: u32,
        blank: bool,
        fault: Option<(u32, StopReason)>,
    }

    #[derive(Clone, Copy)]
    struct Sheet {
        seed: u32,
        width: u32,
        front: Page,
        back: Option<Page>,
    }

    impl Sheet {
        fn duplex(seed: u32, width: u32, height: u32) -> Self {
            let page = Page {
                height,
                blank: false,
                fault: None,
            };
            Self {
                seed,
                width,
                front: page,
                back: Some(page),
            }
        }

        fn front_only(seed: u32, width: u32, height: u32) -> Self {
            Self {
                back: None,
                ..Self::duplex(seed, width, height)
            }
        }

        fn blank_back(mut self) -> Self {
            self.back = self.back.map(|page| Page {
                blank: true,
                ..page
            });
            self
        }

        fn fails_on_front(mut self, row: u32, reason: StopReason) -> Self {
            self.front.fault = Some((row, reason));
            self
        }
    }

    #[derive(Default)]
    struct State {
        feeder: VecDeque<Sheet>,
        current: Option<Sheet>,
        side: Option<Side>,
        row: u32,
        dpi: u32,
        color: Option<ColorMode>,
    }

    #[derive(Clone, Default)]
    struct TestScanner(Arc<Mutex<State>>);

    impl TestScanner {
        fn load(&self, sheet: Sheet) {
            self.0.lock().unwrap().feeder.push_back(sheet);
        }
    }

    impl ScannerDevice for TestScanner {
        fn identity(&self) -> String {
            "Test DR-M260".into()
        }

        fn configure(&mut self, profile: &Profile) -> io::Result<()> {
            let mut state = self.0.lock().unwrap();
            state.dpi = profile.dpi;
            state.color = Some(profile.color);
            Ok(())
        }

        fn next_side(&mut self) -> io::Result<DeviceStep> {
            let mut state = self.0.lock().unwrap();
            let side = match (state.current, state.side) {
                (Some(sheet), Some(Side::Front)) if sheet.back.is_some() => Side::Back,
                _ => {
                    state.current = state.feeder.pop_front();
                    if state.current.is_none() {
                        state.side = None;
                        return Ok(DeviceStep::FeederEmpty);
                    }
                    Side::Front
                }
            };
            state.side = Some(side);
            state.row = 0;
            let width = state.current.unwrap().width;
            Ok(DeviceStep::Side(SideInfo {
                side,
                width,
                dpi: state.dpi,
                color: state.color.unwrap(),
            }))
        }

        fn read_row(&mut self, row: &mut [u8]) -> io::Result<RowStep> {
            let mut state = self.0.lock().unwrap();
            let sheet = state.current.unwrap();
            let side = state.side.unwrap();
            let page = if side == Side::Front {
                sheet.front
            } else {
                sheet.back.unwrap()
            };
            if page.fault.is_some_and(|(at, _)| at == state.row) {
                state.current = None;
                state.side = None;
                return Ok(RowStep::Stopped(page.fault.unwrap().1));
            }
            if state.row == page.height {
                return Ok(RowStep::End);
            }
            if page.blank {
                row.fill(255);
            } else {
                row.copy_from_slice(&pattern(
                    sheet.seed,
                    side,
                    state.row,
                    sheet.width,
                    state.color.unwrap().channels(),
                ));
            }
            state.row += 1;
            Ok(RowStep::Row)
        }

        fn abort_side(&mut self) -> io::Result<()> {
            let mut state = self.0.lock().unwrap();
            state.current = None;
            state.side = None;
            Ok(())
        }

        fn wait_for_paper(&mut self, _timeout_ms: u32) -> io::Result<bool> {
            Ok(!self.0.lock().unwrap().feeder.is_empty())
        }
    }

    fn pattern(seed: u32, side: Side, y: u32, width: u32, channels: usize) -> Vec<u8> {
        let side_value = u32::from(side == Side::Back);
        (0..width)
            .flat_map(|x| {
                (0..channels as u32).map(move |channel| {
                    ((x * 3 + y * 7 + channel * 11 + seed * 13 + side_value * 17) % 251) as u8
                })
            })
            .collect()
    }

    fn space(scenario: &str) -> PathBuf {
        let path = env::current_dir()
            .unwrap()
            .join("target")
            .join("tdd1")
            .join(scenario);
        if path.exists() {
            fs::remove_dir_all(&path).unwrap();
        }
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn app(scanner: &TestScanner) -> OwnScannerApp<TestScanner> {
        OwnScannerApp::with_stamp(scanner.clone(), || STAMP.to_owned())
    }

    fn stop_when_waiting(event: &AppEvent) -> Flow {
        if *event == AppEvent::WaitingForPaper {
            Flow::Stop
        } else {
            Flow::Continue
        }
    }

    #[test]
    fn start_feed_and_every_side_becomes_a_lossless_paired_png() {
        let out = space("feed_and_see");
        let scanner = TestScanner::default();
        scanner.load(Sheet::duplex(1, 40, 90));
        scanner.load(Sheet::duplex(2, 32, 140));
        let mut shown = Vec::new();

        let report = app(&scanner)
            .run_batch(&Profile::long_raw(&out), &mut |event| {
                if let AppEvent::Saved(side) = event {
                    shown.push(side.file.clone());
                }
                stop_when_waiting(event)
            })
            .unwrap();

        assert_eq!(
            shown,
            [
                "scan_20261121161524305_0001_Fron_600_None.png",
                "scan_20261121161524305_0001_Back_600_None.png",
                "scan_20261121161524305_0002_Fron_600_None.png",
                "scan_20261121161524305_0002_Back_600_None.png",
            ]
        );
        assert_png(&out.join(&shown[3]), 2, Side::Back, 32, 140, 3);
        assert_eq!(report.sides[0].sha256.len(), 64);
        let manifest = fs::read_to_string(report.manifest).unwrap();
        assert!(manifest.contains("\"status\": \"finished\""));
        assert!(manifest.contains("\"active_processing\": []"));
    }

    #[test]
    fn adding_paper_continues_the_same_batch() {
        let out = space("add_paper");
        let scanner = TestScanner::default();
        scanner.load(Sheet::duplex(1, 16, 20));
        let feeder = scanner.clone();
        let mut refills = 1;

        let report = app(&scanner)
            .run_batch(&Profile::long_raw(&out), &mut |event| {
                if *event == AppEvent::WaitingForPaper {
                    if refills == 0 {
                        return Flow::Stop;
                    }
                    refills -= 1;
                    feeder.load(Sheet::duplex(2, 16, 30));
                }
                Flow::Continue
            })
            .unwrap();

        assert_eq!(
            report
                .sides
                .iter()
                .map(|side| side.sheet)
                .collect::<Vec<_>>(),
            [1, 1, 2, 2]
        );
        assert_png(
            &out.join("scan_20261121161524305_0002_Back_600_None.png"),
            2,
            Side::Back,
            16,
            30,
            3,
        );
    }

    #[test]
    fn a_paper_problem_is_fixed_physically_and_scanning_continues() {
        let out = space("fix_and_continue");
        let scanner = TestScanner::default();
        scanner.load(Sheet::duplex(1, 16, 20));
        scanner.load(Sheet::duplex(2, 16, 40).fails_on_front(10, StopReason::DoubleFeed));
        let feeder = scanner.clone();
        let mut asked = Vec::new();
        let mut fixed = false;

        let report = app(&scanner)
            .run_batch(&Profile::long_raw(&out), &mut |event| {
                match event {
                    AppEvent::NeedsAttention { reason, message } => {
                        asked.push((*reason, message.clone()))
                    }
                    AppEvent::WaitingForPaper if !fixed => {
                        fixed = true;
                        feeder.load(Sheet::duplex(2, 16, 40));
                    }
                    AppEvent::WaitingForPaper => return Flow::Stop,
                    _ => {}
                }
                Flow::Continue
            })
            .unwrap();

        assert_eq!(
            asked,
            [(
                StopReason::DoubleFeed,
                "Separate the sheets and reload them.".to_owned()
            )]
        );
        assert!(
            out.join("scan_20261121161524305_0002_Fron_600_None_partial.png")
                .exists()
        );
        assert_png(
            &out.join("scan_20261121161524305_0003_Fron_600_None.png"),
            2,
            Side::Front,
            16,
            40,
            3,
        );
        assert_eq!(report.sides.last().unwrap().sheet, 3);
    }

    #[test]
    fn a_failed_paper_pickup_does_not_consume_a_sheet_number() {
        let out = space("failed_pickup");
        let scanner = TestScanner::default();
        scanner.load(Sheet::duplex(1, 16, 20).fails_on_front(0, StopReason::NoMedia));
        let feeder = scanner.clone();
        let mut retried = false;

        let report = app(&scanner)
            .run_batch(&Profile::long_raw(&out), &mut |event| {
                if *event == AppEvent::WaitingForPaper {
                    if retried {
                        return Flow::Stop;
                    }
                    retried = true;
                    feeder.load(Sheet::duplex(1, 16, 20));
                }
                Flow::Continue
            })
            .unwrap();

        assert_eq!(
            report
                .sides
                .iter()
                .map(|side| side.sheet)
                .collect::<Vec<_>>(),
            [1, 1]
        );
        assert_png(
            &out.join("scan_20261121161524305_0001_Fron_600_None.png"),
            1,
            Side::Front,
            16,
            20,
            3,
        );
    }

    #[test]
    fn a_full_length_document_is_complete_beyond_the_jpeg_limit() {
        let out = space("full_length");
        let scanner = TestScanner::default();
        scanner.load(Sheet::front_only(7, 16, 132_000));

        app(&scanner)
            .run_batch(&Profile::long_raw(&out), &mut stop_when_waiting)
            .unwrap();

        assert_png(
            &out.join("scan_20261121161524305_0001_Fron_600_None.png"),
            7,
            Side::Front,
            16,
            132_000,
            3,
        );
    }

    #[test]
    fn reaching_the_maximum_length_keeps_the_partial_image_and_continues() {
        let out = space("length_limit");
        let mut profile = Profile::long_raw(&out);
        profile.dpi = 150;
        profile.max_length_mm = 356;
        let scanner = TestScanner::default();
        scanner.load(Sheet::front_only(4, 8, 3_000));
        scanner.load(Sheet::front_only(5, 8, 20));

        let report = app(&scanner)
            .run_batch(&profile, &mut stop_when_waiting)
            .unwrap();

        assert_eq!(report.sides[0].stop, Some(StopReason::LengthLimitReached));
        assert_eq!(report.sides[0].height, 2_102);
        assert_eq!(report.sides[1].sheet, 2);
    }

    #[test]
    fn a_blank_looking_back_is_kept() {
        let out = space("blank_back");
        let scanner = TestScanner::default();
        scanner.load(Sheet::duplex(3, 24, 50).blank_back());

        let report = app(&scanner)
            .run_batch(&Profile::long_raw(&out), &mut stop_when_waiting)
            .unwrap();

        assert_eq!(report.sides.len(), 2);
        let image = decode_png(&out.join("scan_20261121161524305_0001_Back_600_None.png"));
        assert!(image.pixels.iter().all(|value| *value == 255));
    }

    #[test]
    fn an_existing_batch_is_never_overwritten() {
        let out = space("no_overwrite");
        let scanner = TestScanner::default();
        scanner.load(Sheet::duplex(1, 16, 20));
        let mut scanner_app = app(&scanner);
        scanner_app
            .run_batch(&Profile::long_raw(&out), &mut stop_when_waiting)
            .unwrap();
        let file = out.join("scan_20261121161524305_0001_Fron_600_None.png");
        let original = fs::read(&file).unwrap();

        let second = scanner_app.run_batch(&Profile::long_raw(&out), &mut stop_when_waiting);

        assert_eq!(second.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(file).unwrap(), original);
    }

    #[test]
    fn grayscale_is_saved_losslessly() {
        let out = space("grayscale");
        let mut profile = Profile::long_raw(&out);
        profile.color = ColorMode::Gray8;
        let scanner = TestScanner::default();
        scanner.load(Sheet::front_only(6, 21, 33));

        app(&scanner)
            .run_batch(&profile, &mut stop_when_waiting)
            .unwrap();

        assert_png(
            &out.join("scan_20261121161524305_0001_Fron_600_None.png"),
            6,
            Side::Front,
            21,
            33,
            1,
        );
    }

    #[test]
    fn capture_perfect_files_pair_by_timestamp_order() {
        let dir = space("capture_perfect");
        for name in [
            "scan_20261121163626093_Fron_600_None_0001.jpg",
            "scan_20261121163627038_Back_600_None_0001.jpg",
            "scan_20261121163702827_Fron_600_None_0001.jpg",
            "scan_20261121163704478_Fron_600_None_0001.jpg",
            "scan_20261121163705000_Back_600_None_0001.jpg",
            "scan_20261121163706000_Back_600_None_0001.jpg",
            "notes.txt",
        ] {
            fs::write(dir.join(name), b"not decoded").unwrap();
        }

        let report = pair_capture_perfect(&dir).unwrap();

        assert_eq!(report.sheets.len(), 4);
        assert!(report.sheets[1].back.is_none());
        assert!(
            report.sheets[2]
                .back
                .as_ref()
                .unwrap()
                .contains("163705000")
        );
        assert_eq!(report.orphan_backs.len(), 1);
        assert_eq!(report.ignored, 1);
    }

    #[test]
    fn live_preview_stays_bounded_for_a_full_length_document() {
        let mut pane = Pane::default();
        pane.begin(5184, "Front".into());
        let row = vec![200; 5184 * 3];

        for _ in 0..132_000 {
            pane.push(&row, 3);
        }

        assert!(pane.rows < PREVIEW_MAX_ROWS);
        assert_eq!(pane.pixels.len(), pane.rows * pane.stride);
        assert!(pane.width <= PREVIEW_WIDTH);
    }

    #[test]
    fn the_previous_image_stays_visible_until_the_next_side_is_ready() {
        let mut pane = Pane::default();
        pane.begin(4, "Front".into());
        pane.push(&[10; 12], 3);
        pane.publish();

        pane.begin(4, "Front".into());
        pane.push(&[200; 12], 3);
        assert_eq!(pane.shown_pixels[0], 10);

        pane.publish();
        assert_eq!(pane.shown_pixels[0], 200);
    }

    #[test]
    fn every_session_side_stays_available_and_can_be_reopened() {
        let dir = space("session_images");
        let mut session = Session::default();
        session.open_cache(dir.join("preview.bin")).unwrap();
        let side = |value: u8| {
            let mut pane = Pane::default();
            pane.begin(200, "side".into());
            for _ in 0..30 {
                pane.push(&vec![value; 600], 3);
            }
            pane.publish();
            pane
        };

        session.add(1, Side::Front, "front1.png", &side(10));
        session.add(1, Side::Back, "back1.png", &side(20));
        session.add(2, Side::Front, "front2.png", &side(30));
        session.select(1);

        assert_eq!(session.sheets(), [1, 2]);
        assert_eq!(session.view[0].pixels[0], 10);
        assert_eq!(session.view[1].pixels[0], 20);
        assert!(
            session
                .images
                .iter()
                .all(|image| image.thumb.width <= THUMB_WIDTH)
        );

        session.select(2);
        assert_eq!(session.view[0].pixels[0], 30);
        assert!(session.view[1].pixels.is_empty());

        session.follow_live();
        assert_eq!(session.selected, None);
    }

    #[test]
    fn the_log_lists_front_and_back_on_separate_readable_lines() {
        let side = |side, bytes, sha: &str| SavedSide {
            sheet: 3,
            side,
            status: SideStatus::Complete,
            file: String::new(),
            width: 2744,
            height: 2250,
            bytes,
            sha256: sha.repeat(12),
            stop: None,
            timing: SideTiming::default(),
        };
        let timed = SavedSide {
            timing: SideTiming {
                driver_wait: Duration::from_millis(250),
                transfer: Duration::from_millis(3100),
                compress: Duration::from_millis(8400),
                hash: Duration::from_millis(200),
                publish: Duration::from_millis(3),
            },
            ..side(Side::Front, 1_048_576, "c")
        };
        assert_eq!(
            sheet_log_lines(&[(timed, String::new())], 600, "")[0],
            "sheet 0003 Front: 2744x2250 px, 116x95 mm, 1.00 MB, cccccccccccc, wait 0.25s, transfer 3.10s, compress+write 8.40s, hash 0.20s, publish 0.003s"
        );
        let lines = sheet_log_lines(
            &[
                (side(Side::Front, 2_097_152, "a"), String::new()),
                (side(Side::Back, 1_048_576, "b"), String::new()),
            ],
            600,
            &pending_label(65535),
        );

        assert_eq!(
            lines,
            [
                "sheet 0003 Front: 2744x2250 px, 116x95 mm, 2.00 MB, aaaaaaaaaaaa",
                "sheet 0003 Back : 2744x2250 px, 116x95 mm, 1.00 MB, bbbbbbbbbbbb, pending more",
            ]
        );
    }

    #[test]
    fn image_tags_hide_routine_values_and_explain_useful_ones() {
        assert_eq!(image_tag_note(0x1249, "1", Side::Front), None);
        assert_eq!(image_tag_note(0x1245, "1", Side::Front), None);
        assert_eq!(
            image_tag_note(0x1245, "2", Side::Front).as_deref(),
            Some("driver reports Back side")
        );
        assert_eq!(
            image_tag_note(0x1247, "1", Side::Front).as_deref(),
            Some("image merged = 1")
        );
    }
    #[test]
    #[ignore = "development benchmark for a real raw RGB image"]
    fn dev_encode_real_raw_rgb() {
        let input = env::var("OWNSCANNER_RAW").unwrap();
        let output = env::var("OWNSCANNER_PNG").unwrap();
        let width: u32 = env::var("OWNSCANNER_WIDTH").unwrap().parse().unwrap();
        let bytes = fs::read(input).unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(output)
            .unwrap();
        let mut png = PngStream::new(file, width, ColorMode::Rgb24, 600).unwrap();
        for row in bytes.chunks_exact(width as usize * 3) {
            png.write_row(row).unwrap();
        }
        png.finish().unwrap();
    }

    #[test]
    fn a_jam_is_red_and_opening_then_closing_the_scanner_clears_it() {
        assert_eq!(ScanPhase::Jam.banner(), ("JAM", UI_RED));
        assert_eq!(ScanPhase::Jam.text_color(), UI_WHITE);
        assert_eq!(fault_signal(true, false), FaultSignal::Opened);
        assert_eq!(fault_signal(false, false), FaultSignal::Unchanged);
        assert_eq!(fault_signal(false, true), FaultSignal::Closed);
        assert_eq!(
            phase_when_feeder_unavailable(false),
            Some(ScanPhase::NotReady)
        );
        assert_eq!(phase_when_feeder_unavailable(true), None);
    }

    #[test]
    fn idle_waiting_does_not_send_window_messages_to_the_driver() {
        assert!(!routes_events_to_driver(4));
        assert!(routes_events_to_driver(5));
    }

    #[test]
    fn the_scanner_default_is_chosen_or_the_user_is_asked() {
        let canon = "Canon DR-M200 TWAIN".to_owned();
        let other = "Other Scanner".to_owned();
        let use_source = |name: &str, reason| SourceChoice::Use {
            name: name.to_owned(),
            reason,
        };

        assert_eq!(choose_source(&[], None, None), SourceChoice::None);
        assert_eq!(
            choose_source(std::slice::from_ref(&canon), None, None),
            use_source(&canon, "only scanner found")
        );
        let both = [canon.clone(), other.clone()];
        assert_eq!(
            choose_source(&both, Some(&other), Some(&canon)),
            use_source(&other, "configured")
        );
        assert_eq!(
            choose_source(&both, None, Some(&canon)),
            use_source(&canon, "system default")
        );
        assert!(matches!(
            choose_source(&both, None, None),
            SourceChoice::Ask { .. }
        ));
        assert!(matches!(
            choose_source(&both, Some("Missing"), Some(&canon)),
            SourceChoice::Ask { .. }
        ));
    }

    #[test]
    fn a_chosen_scanner_is_remembered_without_losing_config_lines() {
        let text = "# mine\nschema_version = 1\n\n[scan]\n# scanner = 'example'\noutput_dir = 'x' # keep\n";

        let updated = config_with_scanner(text, "Canon DR-M200 TWAIN").unwrap();
        let replaced = config_with_scanner(&updated, "Other Scanner").unwrap();

        assert!(updated.contains("# mine"));
        assert!(updated.contains("output_dir = 'x' # keep"));
        assert_eq!(
            parse_config(&updated).unwrap().scanner.as_deref(),
            Some("Canon DR-M200 TWAIN")
        );
        assert_eq!(
            parse_config(&replaced).unwrap().scanner.as_deref(),
            Some("Other Scanner")
        );
        assert_eq!(replaced.matches("\nscanner =").count(), 1);
    }

    #[test]
    fn vendor_help_is_optional_and_generic_flow_stays_vendor_neutral() {
        let canon = device_profile("Canon Electronics", "Canon DR-M200 TWAIN").unwrap();
        assert_eq!(canon.name, "Canon DR-M");
        assert!(canon.help(StopReason::Jam).unwrap().contains("OPEN lever"));
        assert!(device_profile("Other Vendor", "Other Scanner").is_none());
        assert!(!StopReason::Jam.instruction().contains("OPEN lever"));
        assert_eq!(TestScanner::default().instruction(StopReason::Jam), None);
    }

    #[test]
    fn space_toggles_pause_once_while_held() {
        assert!(is_space_key(0x0100, 0x20));
        assert!(!is_space_key(0x0101, 0x20));
        assert!(!is_space_key(0x0100, 0x41));
        assert!(is_first_key_press(1));
        assert!(!is_first_key_press(1 << 30 | 1));
    }

    #[test]
    fn read_ahead_is_on_by_default_and_can_be_configured() {
        assert!(Profile::long_raw("scans").read_ahead);
        assert!(
            parse_config("schema_version = 1\n[scan]\n")
                .unwrap()
                .read_ahead
        );
        assert!(
            !parse_config("schema_version = 1\n[scan]\nread_ahead = false\n")
                .unwrap()
                .read_ahead
        );
        assert!(parse_config("schema_version = 1\n[scan]\nread_ahead = 1\n").is_err());
    }

    #[test]
    fn long_document_mode_uses_the_shortest_mode_that_fits() {
        let modes = [0, 1000, 3000, 5588];
        assert_eq!(long_document_mode(&modes, 356), Some(0));
        assert_eq!(long_document_mode(&modes, 900), Some(1000));
        assert_eq!(long_document_mode(&modes, 5588), Some(5588));
        assert_eq!(long_document_mode(&modes, 9000), Some(5588));
        assert_eq!(long_document_mode(&[], 5588), None);
    }

    #[test]
    fn zoom_fits_all_width_height_and_normal() {
        let mut zoom = ViewZoom::default();
        assert_eq!(zoom.scale_for(400, 2000, 800, 1000), 0.5);
        zoom.apply(ZoomAction::Width);
        assert_eq!(zoom.scale_for(400, 2000, 800, 1000), 2.0);
        zoom.apply(ZoomAction::Height);
        assert_eq!(zoom.scale_for(400, 2000, 800, 1000), 0.5);
        zoom.apply(ZoomAction::Normal);
        assert_eq!(zoom.scale_for(400, 2000, 800, 1000), 1.0);
        assert!(zoom.is_active(ZoomAction::Normal));
    }

    #[test]
    fn zoom_in_and_out_keep_position_and_fit_modes_reset_it() {
        let mut zoom = ViewZoom {
            scale: 1.0,
            pan_y: 120,
            ..ViewZoom::default()
        };
        zoom.apply(ZoomAction::In);
        assert_eq!(
            (zoom.mode, zoom.scale, zoom.pan_y),
            (ZoomMode::Custom, 1.25, 120)
        );
        zoom.apply(ZoomAction::Out);
        assert_eq!(zoom.scale, 1.0);
        zoom.apply(ZoomAction::Full);
        assert_eq!((zoom.mode, zoom.pan_y), (ZoomMode::Full, 0));
        for _ in 0..100 {
            zoom.apply(ZoomAction::In);
        }
        assert_eq!(zoom.scale, ZOOM_MAX);
    }

    #[test]
    fn deskew_view_finds_and_corrects_a_rotated_text_block() {
        let (width, rows) = (300, 260);
        let stride = (width * 3 + 3) & !3;
        let mut base = vec![255u8; stride * rows];
        for y in (30..230).step_by(14) {
            for line in y..y + 4 {
                for x in 40..260 {
                    if (x / 9) % 4 != 0 {
                        base[line * stride + x * 3..line * stride + x * 3 + 3].fill(20);
                    }
                }
            }
        }

        let tilted = rotate_bgr(&base, width, rows, 12.0);
        let angle = detect_skew(&tilted.pixels, tilted.width, tilted.rows).unwrap();

        assert!((angle + 12.0).abs() < 0.7, "angle {angle}");
        let straight = rotate_bgr(&tilted.pixels, tilted.width, tilted.rows, angle);
        let again = detect_skew(&straight.pixels, straight.width, straight.rows).unwrap();
        assert!(again.abs() < 0.7, "remaining angle {again}");
    }

    #[test]
    fn deskew_view_reports_no_angle_for_blank_pages() {
        let stride = (100 * 3 + 3) & !3;
        assert_eq!(detect_skew(&vec![255; stride * 80], 100, 80), None);
    }

    #[test]
    fn zoom_stops_at_400_percent_and_reports_its_level() {
        let mut zoom = ViewZoom::default();
        zoom.apply(ZoomAction::Normal);
        zoom.scale = 1.0;
        for _ in 0..30 {
            zoom.apply(ZoomAction::In);
        }
        assert_eq!(zoom.scale_for(100, 100, 500, 500), 4.0);
        assert_eq!(zoom_percent(zoom.scale), "400%");
        zoom.apply(ZoomAction::Width);
        assert_eq!(zoom.scale_for(10, 10, 500, 500), 4.0);
    }

    #[test]
    fn scanner_deskew_is_off_by_default_and_can_be_enabled() {
        assert!(!Profile::long_raw("scans").deskew);
        let profile = parse_config("schema_version = 1\n[scan]\ndeskew = true\n").unwrap();
        assert!(profile.deskew);
        assert_eq!(profile.active_processing(), ["deskew"]);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_mouse() {
        let mut zoom = ViewZoom {
            mode: ZoomMode::Custom,
            scale: 1.0,
            pan_x: 100,
            pan_y: 200,
            ..ViewZoom::default()
        };

        zoom.zoom_at(ZoomAction::In, 50, 60);

        assert_eq!((zoom.scale, zoom.pan_x, zoom.pan_y), (1.25, 138, 265));
        let content_x = f64::from(zoom.pan_x + 50) / zoom.scale;
        let content_y = f64::from(zoom.pan_y + 60) / zoom.scale;
        assert!((content_x - 150.0).abs() <= 0.5);
        assert!((content_y - 260.0).abs() <= 0.5);
    }

    #[test]
    fn scroll_indicator_shows_position_and_visible_share() {
        assert_eq!(scroll_thumb(100, 400, 0), (0, 25));
        assert_eq!(scroll_thumb(100, 400, 300), (75, 25));
        assert_eq!(scroll_thumb(100, 100, 0), (0, 100));
        assert_eq!(scroll_thumb(100, 10_000, 0).1, 18);
    }

    #[test]
    fn full_resolution_decoder_reads_our_png_exactly() {
        let dir = space("full_decode");
        for (name, color, channels) in [
            ("rgb.png", ColorMode::Rgb24, 3),
            ("gray.png", ColorMode::Gray8, 1),
        ] {
            let path = dir.join(name);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            let mut png = PngStream::new(file, 37, color, 600).unwrap();
            for y in 0..200 {
                png.write_row(&pattern(9, Side::Front, y, 37, channels))
                    .unwrap();
            }
            png.finish().unwrap();

            let image = decode_png_bgr(&path).unwrap();

            assert_eq!((image.width, image.rows), (37, 200));
            let stride = (37 * 3 + 3) & !3;
            for y in 0..200usize {
                let row = pattern(9, Side::Front, y as u32, 37, channels);
                for x in 0..37 {
                    let (red, green, blue) = if channels == 3 {
                        (row[x * 3], row[x * 3 + 1], row[x * 3 + 2])
                    } else {
                        (row[x], row[x], row[x])
                    };
                    let at = y * stride + x * 3;
                    assert_eq!(&image.pixels[at..at + 3], &[blue, green, red]);
                }
            }
        }
    }

    #[test]
    fn inflate_reads_fixed_and_stored_blocks() {
        assert_eq!(
            inflate_raw(&[0x4b, 0x4c, 0x4a, 0x06, 0x00], 3).unwrap(),
            b"abc"
        );
        assert_eq!(
            inflate_raw(&[0x01, 3, 0, 0xfc, 0xff, b'a', b'b', b'c'], 3).unwrap(),
            b"abc"
        );
        assert!(inflate_raw(&[0x4b, 0x4c, 0x4a, 0x06, 0x00], 2).is_err());
    }

    #[test]
    fn double_click_inspects_real_pixels_and_returns_to_the_fit_view() {
        let mut zoom = ViewZoom {
            mode: ZoomMode::Width,
            fit: ZoomMode::Width,
            scale: 0.5,
            pan_x: 0,
            pan_y: 100,
        };

        zoom.toggle_inspect(50, 60);
        assert_eq!(
            (zoom.mode, zoom.scale, zoom.pan_x, zoom.pan_y),
            (ZoomMode::Normal, 1.0, 50, 260)
        );

        zoom.toggle_inspect(50, 60);
        assert_eq!((zoom.mode, zoom.pan_x, zoom.pan_y), (ZoomMode::Width, 0, 0));
    }

    #[test]
    fn the_fit_mode_is_remembered_as_readable_state() {
        for mode in [ZoomMode::Full, ZoomMode::Width, ZoomMode::Height] {
            assert_eq!(parse_fit_mode(fit_mode_text(mode)), mode);
        }
        assert_eq!(parse_fit_mode("unknown"), ZoomMode::Width);
    }

    #[test]
    #[ignore = "development timing of save stages on a real scan, without a scanner"]
    fn dev_time_save_stages() {
        let image = decode_png_bgr(Path::new(&env::var("OWNSCANNER_PNG").unwrap())).unwrap();
        let path = PathBuf::from(env::var("OWNSCANNER_OUT").unwrap());
        let stride = (image.width * 3 + 3) & !3;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let started = std::time::Instant::now();
        let mut png = PngStream::new(file, image.width as u32, ColorMode::Rgb24, 600).unwrap();
        let mut row = vec![0u8; image.width * 3];
        for y in 0..image.rows {
            let source = &image.pixels[y * stride..y * stride + image.width * 3];
            for (target, pixel) in row.chunks_exact_mut(3).zip(source.chunks_exact(3)) {
                target.copy_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
            png.write_row(&row).unwrap();
        }
        png.finish().unwrap();
        let encoded = started.elapsed();
        let started = std::time::Instant::now();
        sha256_file(&path).unwrap();
        let hashed = started.elapsed();
        let bytes = fs::read(&path).unwrap();
        let copy = path.with_extension("copy");
        let started = std::time::Instant::now();
        let mut output = File::create(&copy).unwrap();
        output.write_all(&bytes).unwrap();
        output.sync_all().unwrap();
        let written = started.elapsed();
        drop(output);
        let _ = fs::remove_file(&copy);
        let _ = fs::remove_file(&path);
        println!(
            "{}x{} raw {:.0} MB, png {:.1} MB: encode+stream {:.2}s, sha256 {:.2}s, plain disk write+sync {:.3}s",
            image.width,
            image.rows,
            (image.width * image.rows * 3) as f64 / 1e6,
            bytes.len() as f64 / 1e6,
            encoded.as_secs_f64(),
            hashed.as_secs_f64(),
            written.as_secs_f64()
        );
    }

    #[test]
    #[ignore = "development check against a real PNG and ImageMagick raw RGB"]
    fn dev_decode_real_png() {
        let image = decode_png_bgr(Path::new(&env::var("OWNSCANNER_PNG").unwrap())).unwrap();
        let raw = fs::read(env::var("OWNSCANNER_RAW").unwrap()).unwrap();
        let stride = (image.width * 3 + 3) & !3;
        assert_eq!(raw.len(), image.width * image.rows * 3);
        for y in 0..image.rows {
            for x in 0..image.width {
                let at = y * stride + x * 3;
                let source = (y * image.width + x) * 3;
                assert_eq!(
                    [image.pixels[at + 2], image.pixels[at + 1], image.pixels[at]],
                    [raw[source], raw[source + 1], raw[source + 2]]
                );
            }
        }
    }

    #[test]
    fn config_values_show_current_and_driver_allowed_values() {
        let driver = parse_driver_values(
            "source: Test Scanner (system default)\nselected: Test Scanner (only scanner found)\nICAP_PIXELTYPE [0x0101] current=2 options=0,1,2\nICAP_XRESOLUTION [0x1118] current=600.000 options=300.000,600.000\nstandard [0x1112] current=220.000 options=220.000\n",
        );
        let profile = Profile::long_raw("scans");
        let rows = config_value_rows(&profile, Some(&driver));

        let text = format_config_values(&rows, Some("dpi")).unwrap();
        assert!(text.contains("600") && text.contains("300, 600"));
        let all = format_config_values(&rows, None).unwrap();
        assert!(all.contains("rgb24, gray8"));
        assert!(all.contains("1..5588"));
        assert!(all.contains("Test Scanner"));
        assert_eq!(driver.selected.as_deref(), Some("Test Scanner"));

        let generic = config_value_rows(&profile, None);
        assert!(
            format_config_values(&generic, Some("dpi"))
                .unwrap()
                .contains("positive whole number")
        );
        assert!(
            format_config_values(&rows, Some("nope"))
                .unwrap_err()
                .contains("use one of: output_dir")
        );
    }

    #[test]
    fn progress_shows_saved_work_and_the_sheet_in_progress() {
        assert_eq!(
            progress_text(&[1, 2, 3], 6, Some(4), "more"),
            (
                "Saved 3 sheets (6 images)".to_owned(),
                "Processing sheet 4 · more".to_owned()
            )
        );
        assert_eq!(
            progress_text(&[1, 2, 3, 4], 7, Some(4), "0"),
            (
                "Saved 3 sheets (7 images)".to_owned(),
                "Processing sheet 4".to_owned()
            )
        );
        assert_eq!(
            progress_text(&[1], 2, None, "0"),
            ("Saved 1 sheet (2 images)".to_owned(), String::new())
        );
        assert_eq!(progress_text(&[1], 2, None, "more").1, "More in scanner");
    }

    #[test]
    fn zoom_group_is_right_aligned_and_deskew_is_separate() {
        let panel = zoom_panel_rect(1200);
        assert_eq!(panel.right, 1200);
        for index in 0..6 {
            let rect = zoom_button_rect(index, 1200);
            assert!(rect.left > panel.left && rect.right <= zoom_percent_x(1200));
        }
        assert!(zoom_percent_x(1200) < panel.right);
        let deskew = zoom_button_rect(6, 1200);
        assert!(deskew.right < panel.left);
        assert!(deskew.left > PAUSE_RECT.right);
        assert_eq!(zoom_panel_rect(200).right, MIN_TOOLBAR_RIGHT);
    }

    #[test]
    fn closing_waits_until_no_image_is_in_flight() {
        let blocked = |phase, sheet, pending| close_blocked_reason(phase, sheet, pending, false);
        assert!(blocked(ScanPhase::Scanning, Some(4), "more").is_some());
        assert!(blocked(ScanPhase::Scanned, None, "more").is_some());
        assert!(blocked(ScanPhase::Scanned, None, "").is_some());
        assert!(blocked(ScanPhase::Paused, None, "more").is_some());
        for stable in [
            ScanPhase::Starting,
            ScanPhase::WaitingForLock,
            ScanPhase::WaitingForPaper,
            ScanPhase::Paused,
            ScanPhase::Jam,
            ScanPhase::Attention,
            ScanPhase::NotReady,
            ScanPhase::Failed,
        ] {
            assert_eq!(blocked(stable, None, ""), None, "{stable:?}");
        }
        assert_eq!(
            close_blocked_reason(ScanPhase::Scanning, Some(4), "more", true),
            None,
            "a silent driver must not trap the user"
        );
    }

    #[test]
    fn a_problem_during_the_final_save_cancels_the_close() {
        for problem in [
            ScanPhase::Jam,
            ScanPhase::Attention,
            ScanPhase::NotReady,
            ScanPhase::Failed,
        ] {
            assert!(close_cancelled_by(problem), "{problem:?}");
        }
        for normal in [
            ScanPhase::Scanning,
            ScanPhase::Scanned,
            ScanPhase::Paused,
            ScanPhase::WaitingForPaper,
        ] {
            assert!(!close_cancelled_by(normal), "{normal:?}");
        }
    }

    #[test]
    fn only_a_pickup_that_captured_nothing_can_return_to_waiting() {
        let clean =
            |reason, bytes, duplex, next| failed_before_any_image(reason, bytes, duplex, next);
        assert!(clean(StopReason::NoMedia, 0, true, Side::Back));
        assert!(clean(StopReason::NoMedia, 0, false, Side::Front));
        assert!(
            !clean(StopReason::NoMedia, 0, true, Side::Front),
            "back is missing"
        );
        assert!(
            !clean(StopReason::NoMedia, 5544, true, Side::Back),
            "partial image"
        );
        assert!(!clean(StopReason::Jam, 0, true, Side::Back));
        assert!(!clean(StopReason::DoubleFeed, 0, true, Side::Back));
    }

    #[test]
    fn the_log_identifies_the_exact_running_build() {
        let identity = build_identity();
        let build = identity
            .strip_prefix("build ")
            .and_then(|rest| rest.split(';').next())
            .unwrap();
        assert_eq!(build.len(), 12, "{identity}");
        assert!(build.chars().all(|c| c.is_ascii_hexdigit()), "{identity}");
        assert!(identity.contains("; exe "), "{identity}");
    }

    #[test]
    fn retry_appears_only_after_a_paper_stop() {
        assert!(!retry_visible(ScanPhase::WaitingForPaper));
        assert!(!retry_visible(ScanPhase::Scanning));
        assert!(!retry_visible(ScanPhase::NotReady));
        assert!(retry_visible(ScanPhase::Jam));
        assert!(retry_visible(ScanPhase::Attention));
    }

    #[test]
    fn a_second_click_on_the_pinned_sheet_returns_to_live() {
        let mut pane = Pane::default();
        pane.begin(8, "side".into());
        pane.push(&[10; 24], 3);
        pane.publish();
        let mut session = Session::default();
        session.add(1, Side::Front, "one.png", &pane);
        session.add(2, Side::Front, "two.png", &pane);

        session.toggle_sheet(1);
        assert_eq!((session.selected, session.pinned), (Some(1), true));
        session.toggle_sheet(2);
        assert_eq!(session.selected, Some(2));
        session.toggle_sheet(2);
        assert_eq!((session.selected, session.pinned), (None, false));
    }

    #[test]
    fn the_delete_icon_is_inside_the_selected_sheet_row() {
        let icon = sheet_delete_rect(100);
        assert!(rect_contains(&icon, UI_STRIP_WIDTH - 20, 110));
        assert!(!rect_contains(&icon, 20, 110));
        assert!(!rect_contains(&icon, UI_STRIP_WIDTH - 20, 140));
    }

    #[test]
    fn deleting_a_sheet_moves_both_sides_to_deleted_without_overwriting() {
        let dir = space("delete_sheet");
        let side = || {
            let mut pane = Pane::default();
            pane.begin(8, "side".into());
            pane.push(&[10; 24], 3);
            pane.publish();
            pane
        };
        for name in ["front.png", "back.png", "other.png"] {
            fs::write(dir.join(name), name).unwrap();
        }
        fs::create_dir_all(dir.join(".deleted")).unwrap();
        fs::write(dir.join(".deleted/front.png"), "older").unwrap();
        let mut session = Session::default();
        session.add(1, Side::Front, "front.png", &side());
        session.add(1, Side::Back, "back.png", &side());
        session.add(2, Side::Front, "other.png", &side());
        session.select(1);

        let moved = session.delete_sheet(1, &dir).unwrap();

        assert_eq!(moved, ["front.png", "back.png"]);
        assert!(!dir.join("front.png").exists() && !dir.join("back.png").exists());
        assert_eq!(
            fs::read_to_string(dir.join(".deleted/front.png")).unwrap(),
            "older"
        );
        assert_eq!(
            fs::read_to_string(dir.join(".deleted/front.png.1")).unwrap(),
            "front.png"
        );
        assert_eq!(
            fs::read_to_string(dir.join(".deleted/back.png")).unwrap(),
            "back.png"
        );
        assert!(dir.join("other.png").exists());
        assert_eq!(session.sheets(), [2]);
        assert_eq!(session.selected, None);
    }

    #[test]
    fn device_events_explain_paper_faults() {
        assert_eq!(device_event_name(13), "paper jam");
        assert_eq!(device_event_fault(13), Some(StopReason::Jam));
        assert_eq!(device_event_fault(12), Some(StopReason::DoubleFeed));
        assert_eq!(device_event_fault(8), None);
        assert_eq!(device_event_name(999), "not mapped yet");
    }

    #[test]
    fn config_follows_forced_local_xdg_then_system_precedence() {
        let root = space("config_precedence");
        let (work, xdg, system) = (root.join("work"), root.join("xdg"), root.join("system"));
        for dir in [&work, &xdg.join("ownscanner"), &system.join("ownscanner")] {
            fs::create_dir_all(dir).unwrap();
        }
        let write = |path: PathBuf, folder: &str| {
            fs::write(
                path,
                format!("schema_version = 1\n[scan]\noutput_dir = '{folder}'\n"),
            )
            .unwrap();
        };
        let lookup = |name: &str| match name {
            "XDG_CONFIG_HOME" => Some(xdg.display().to_string()),
            "ProgramData" => Some(system.display().to_string()),
            _ => None,
        };

        write(system.join("ownscanner/config.toml"), "from-system");
        assert_eq!(
            load_profile(None, &work, &lookup).unwrap().0.directory,
            Path::new("from-system")
        );
        write(xdg.join("ownscanner/config.toml"), "from-xdg");
        assert_eq!(
            load_profile(None, &work, &lookup).unwrap().0.directory,
            Path::new("from-xdg")
        );
        write(work.join(".ownscanner.toml"), "from-local");
        assert_eq!(
            load_profile(None, &work, &lookup).unwrap().0.directory,
            Path::new("from-local")
        );
        let forced = root.join("forced.toml");
        write(forced.clone(), "from-forced");
        assert_eq!(
            load_profile(Some(&forced), &work, &lookup)
                .unwrap()
                .0
                .directory,
            Path::new("from-forced")
        );
        assert!(load_profile(Some(&root.join("missing.toml")), &work, &lookup).is_err());
    }

    #[test]
    fn config_defaults_to_dot_config_and_rejects_unknown_settings() {
        let lookup = |name: &str| (name == "HOME").then(|| "C:/Users/test".to_owned());
        assert_eq!(
            global_config_path(&lookup).unwrap(),
            Path::new("C:/Users/test/.config/ownscanner/config.toml")
        );
        assert!(
            parse_config("[scan]\ndpi = 600\n")
                .unwrap_err()
                .contains("schema_version")
        );
        assert!(
            parse_config("schema_version = 1\n[scan]\nsharpen = true\n")
                .unwrap_err()
                .contains("unknown setting")
        );
        assert!(parse_config("schema_version = 1\n[scan]\ndpi = 0\n").is_err());
    }

    #[test]
    fn config_init_creates_a_valid_file_and_never_overwrites_it() {
        let root = space("config_init");
        let path = root.join("config/ownscanner/config.toml");

        init_config(&path, &root.join("scans")).unwrap();

        let profile = parse_config(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(profile.directory, root.join("scans"));
        assert_eq!(profile.max_length_mm, 5588);
        assert_eq!(
            init_config(&path, &root.join("other")).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn the_status_banner_names_the_scan_states() {
        assert_eq!(ScanPhase::WaitingForPaper.banner(), ("WAITING", UI_HONEY));
        assert_eq!(ScanPhase::Paused.banner().1, UI_HONEY);
        assert_eq!(ScanPhase::Starting.banner().1, UI_SAGE);
        assert_eq!(ScanPhase::WaitingForLock.banner().1, UI_RED);
        assert_eq!(ScanPhase::Scanning.banner().1, UI_SAGE);
        assert_eq!(ScanPhase::Scanned.banner().1, UI_SAGE);
        assert_eq!(ScanPhase::NotReady.banner(), ("NOT READY", UI_RED));
        assert_eq!(ScanPhase::Scanning.banner().0, "SCANNING");
        assert_eq!(ScanPhase::Scanned.banner().0, "SCANNED");
        assert_eq!(ScanPhase::Attention.banner().0, "ATTENTION");
    }

    #[test]
    fn log_lines_show_time_area_and_explanation() {
        assert_eq!(
            trace_line("20261121172515468", "feeder", "paper detected"),
            "17:25:15.468 [feeder] paper detected"
        );
    }

    #[test]
    fn sha256_matches_the_standard_vector() {
        let dir = space("sha256");
        let file = dir.join("abc.txt");
        fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha256_file(&file).unwrap().0,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn cli_wiring_reports_version_help_and_usage_errors() {
        let call = |args: &[&str]| {
            let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = run_cli(&args, &mut out, &mut err);
            (
                code,
                String::from_utf8(out).unwrap(),
                String::from_utf8(err).unwrap(),
            )
        };
        assert_eq!(
            call(&["--version"]).1.trim(),
            format!("ownscanner {VERSION}")
        );
        assert!(call(&["--help"]).2.contains("pair <folder>"));
        let (code, _, err) = call(&["unknown"]);
        assert_eq!(code, 2);
        assert!(err.contains("unknown command"));
        for args in [["scan", "--help"], ["scan", "-h"], ["pair", "--help"]] {
            let (code, _, err) = call(&args);
            assert_eq!(code, 0);
            assert!(err.contains("Usage:"));
        }
        let (code, _, err) = call(&["scan", "--typo"]);
        assert_eq!(code, 2);
        assert!(err.contains("unknown option `--typo`"));
    }

    struct Decoded {
        width: u32,
        height: u32,
        channels: usize,
        ppm: u32,
        pixels: Vec<u8>,
    }

    fn assert_png(path: &Path, seed: u32, side: Side, width: u32, height: u32, channels: usize) {
        let image = decode_png(path);
        assert_eq!(
            (image.width, image.height, image.channels, image.ppm),
            (width, height, channels, 23_622)
        );
        for (y, row) in image
            .pixels
            .chunks_exact(width as usize * channels)
            .enumerate()
        {
            assert_eq!(
                row,
                pattern(seed, side, y as u32, width, channels),
                "row {y}"
            );
        }
    }

    fn decode_png(path: &Path) -> Decoded {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let (mut at, mut idat) = (8, Vec::new());
        let (mut width, mut height, mut channels, mut ppm) = (0, 0, 0, 0);
        loop {
            let length = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &bytes[at + 4..at + 8];
            let data = &bytes[at + 8..at + 8 + length];
            let crc =
                u32::from_be_bytes(bytes[at + 8 + length..at + 12 + length].try_into().unwrap());
            assert_eq!(
                crc32(crc32_update(crc32_update(0xffff_ffff, kind), data)),
                crc
            );
            match kind {
                b"IHDR" => {
                    width = u32::from_be_bytes(data[..4].try_into().unwrap());
                    height = u32::from_be_bytes(data[4..8].try_into().unwrap());
                    channels = if data[9] == 2 { 3 } else { 1 };
                }
                b"pHYs" => ppm = u32::from_be_bytes(data[..4].try_into().unwrap()),
                b"IDAT" => idat.extend_from_slice(data),
                b"IEND" => break,
                _ => {}
            }
            at += 12 + length;
        }
        let raw = inflate(&idat[2..idat.len() - 4]);
        let (mut a, mut b) = (1u32, 0u32);
        for byte in &raw {
            a = (a + u32::from(*byte)) % 65521;
            b = (b + a) % 65521;
        }
        assert_eq!(idat[idat.len() - 4..], ((b << 16) | a).to_be_bytes());
        let stride = width as usize * channels;
        let mut pixels = vec![0; stride * height as usize];
        for y in 0..height as usize {
            let line = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
            for x in 0..stride {
                let left = if x >= channels {
                    pixels[y * stride + x - channels]
                } else {
                    0
                };
                let up = if y > 0 {
                    pixels[(y - 1) * stride + x]
                } else {
                    0
                };
                let up_left = if y > 0 && x >= channels {
                    pixels[(y - 1) * stride + x - channels]
                } else {
                    0
                };
                let prediction = match line[0] {
                    0 => 0,
                    1 => left,
                    2 => up,
                    3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                    4 => paeth(left, up, up_left),
                    other => panic!("invalid PNG filter {other}"),
                };
                pixels[y * stride + x] = line[x + 1].wrapping_add(prediction);
            }
        }
        Decoded {
            width,
            height,
            channels,
            ppm,
            pixels,
        }
    }

    struct TestBits<'a> {
        data: &'a [u8],
        bit: usize,
    }

    impl TestBits<'_> {
        fn take(&mut self, count: usize) -> usize {
            let mut value = 0;
            for index in 0..count {
                value |= usize::from((self.data[self.bit / 8] >> (self.bit % 8)) & 1) << index;
                self.bit += 1;
            }
            value
        }

        fn symbol(&mut self, table: &std::collections::HashMap<(u8, u32), usize>) -> usize {
            let (mut length, mut code) = (0u8, 0u32);
            loop {
                code = (code << 1) | self.take(1) as u32;
                length += 1;
                if let Some(symbol) = table.get(&(length, code)) {
                    return *symbol;
                }
                assert!(length <= 15, "invalid Huffman code");
            }
        }
    }

    fn test_table(lengths: &[u8]) -> std::collections::HashMap<(u8, u32), usize> {
        canonical_codes(lengths)
            .into_iter()
            .enumerate()
            .filter(|(symbol, _)| lengths[*symbol] != 0)
            .map(|(symbol, code)| ((lengths[symbol], code), symbol))
            .collect()
    }

    fn inflate(data: &[u8]) -> Vec<u8> {
        const ORDER: [usize; 19] = [
            16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
        ];
        let mut bits = TestBits { data, bit: 0 };
        let mut out = Vec::new();
        loop {
            let last = bits.take(1) == 1;
            assert_eq!(bits.take(2), 0b10);
            let literal_count = bits.take(5) + 257;
            let distance_count = bits.take(5) + 1;
            let code_count = bits.take(4) + 4;
            let mut code_lengths = [0u8; 19];
            for symbol in &ORDER[..code_count] {
                code_lengths[*symbol] = bits.take(3) as u8;
            }
            let code_table = test_table(&code_lengths);
            let mut lengths = Vec::new();
            while lengths.len() < literal_count + distance_count {
                match bits.symbol(&code_table) {
                    16 => {
                        let previous = *lengths.last().unwrap();
                        let count = 3 + bits.take(2);
                        lengths.extend(std::iter::repeat_n(previous, count));
                    }
                    17 => {
                        let count = 3 + bits.take(3);
                        lengths.extend(std::iter::repeat_n(0, count));
                    }
                    18 => {
                        let count = 11 + bits.take(7);
                        lengths.extend(std::iter::repeat_n(0, count));
                    }
                    value => lengths.push(value as u8),
                }
            }
            let literal_table = test_table(&lengths[..literal_count]);
            let distance_table = test_table(&lengths[literal_count..]);
            loop {
                match bits.symbol(&literal_table) {
                    symbol @ 0..=255 => out.push(symbol as u8),
                    256 => break,
                    symbol => {
                        let index = symbol - 257;
                        let length =
                            usize::from(LEN_BASE[index]) + bits.take(usize::from(LEN_EXTRA[index]));
                        let index = bits.symbol(&distance_table);
                        let distance = usize::from(DIST_BASE[index])
                            + bits.take(usize::from(DIST_EXTRA[index]));
                        for _ in 0..length {
                            out.push(out[out.len() - distance]);
                        }
                    }
                }
            }
            if last {
                return out;
            }
        }
    }
}
