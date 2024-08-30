use std::{cmp::min, fs::File, io::Write, path::PathBuf, thread::sleep, time::Duration};

use console::{Emoji, StyledObject};
use futures_util::StreamExt;
use indicatif::{style::TemplateError, ProgressBar, ProgressStyle};
use reqwest::Client;
use sevenz_rust::Password;
use steamlocate::SteamDir;

static STEAM_APP_ID: u32 = 289070;
static DLC_URL: &'static str = "https://pixeldrain.com/api/file/Csbg5SqZ?download";
static PASSWORD: Option<&'static str> = Some("cs.rin.ru");
static ZIP_FILE: &'static str = "dlc.7z";
static DELETE_AFTER: bool = true;

static LOOKING_GLASS: Emoji<'_, '_> = Emoji("🔍", "");
static TRUCK: Emoji<'_, '_> = Emoji("🚚", "");
static CLIP: Emoji<'_, '_> = Emoji("🔗", "");
static PAPER: Emoji<'_, '_> = Emoji("📃", "");
static SPARKLE: Emoji<'_, '_> = Emoji("✨", "");

/// All of the possible errors that could occur.
#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("the steam install directory could not be found")]
    SteamNotFound,
    #[error("the civ6 install directory could not be found")]
    Civ6NotFound,
    #[error("failed to find the parent directory of the civ6 install directory")]
    Civ6NoParent,

    #[error("failed to download the dlc: {0}")]
    DownloadDlc(#[from] reqwest::Error),
    #[error("failed to get content length")]
    ContentLength,
    #[error("failed to create file for the dlc zip: {0}")]
    CreateFile(std::io::Error),
    #[error("failed to download a chunk: {0}")]
    DownloadChunk(std::io::Error),

    #[error("failed to get the length of the dlc zip: {0}")]
    LengthDlc(std::io::Error),
    #[error("failed to create template")]
    Template(#[from] TemplateError),
    #[error("no parent directory found for the 7z file")]
    NoParent7z,
    #[error("failed to extract the dlc: {0}")]
    ExtractDlc(#[from] sevenz_rust::Error),

    #[error("an io error occurred: {0}")]
    Io(#[from] std::io::Error),
}

/// Transforms the progress to bold and dim.
fn progress_style(progress: &'static str) -> StyledObject<&'static str> {
    console::style(progress).bold().dim()
}

/// Show a spinner for a given length in seconds and tick rate.
fn spinner(pb: &ProgressBar, length: u64, ticks: u64) {
    for _ in 0..length / ticks {
        pb.tick();
        sleep(Duration::from_millis(ticks));
    }
}

/// Grab the Civ6 install directory.
fn civ6_install(progress: &'static str) -> Result<PathBuf, Error> {
    let pb = ProgressBar::new_spinner();

    pb.set_message(format!(
        "{} {} Finding Steam install directory...",
        progress_style(progress),
        LOOKING_GLASS
    ));
    let mut steam_dir = SteamDir::locate().ok_or(Error::SteamNotFound)?;
    spinner(&pb, 500, 10);
    pb.set_message(format!(
        "{} {} Found Steam install directory!",
        progress_style(progress),
        SPARKLE
    ));

    pb.set_message(format!(
        "{} {} Finding Civ6 install directory...",
        progress_style(progress),
        LOOKING_GLASS
    ));
    let civ6 = steam_dir.app(&STEAM_APP_ID).ok_or(Error::Civ6NotFound)?;
    let path = civ6.path.parent().ok_or(Error::Civ6NoParent)?.to_path_buf();
    spinner(&pb, 500, 10);
    pb.finish_and_clear();
    println!(
        "{} {} Found Civ6 install directory!",
        progress_style(progress),
        SPARKLE
    );

    Ok(path)
}

/// Download a file from a URL.
///
/// Stolen from: https://gist.github.com/Tapanhaz/096e299bf060607b572d700e89a62529 (with changes)
async fn download_file(
    client: &Client,
    url: &str,
    path: &str,
    start: String,
    done: String,
) -> Result<File, Error> {
    let res = client.get(url).send().await?;
    let total_size = res.content_length().ok_or(Error::ContentLength)?;

    // Indicatif setup
    let pb = ProgressBar::new(total_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{msg}\n{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")?
        .progress_chars("#>-"));
    pb.set_message(start);

    // Download all of the chunks
    let mut file = File::create(path).map_err(Error::CreateFile)?;
    let mut downloaded: u64 = 0;
    let mut stream = res.bytes_stream();

    while let Some(item) = stream.next().await {
        let chunk = item?;
        file.write_all(&chunk).map_err(Error::DownloadChunk)?;
        let new = min(downloaded + (chunk.len() as u64), total_size);
        downloaded = new;
        pb.set_position(new);
    }

    // Done
    pb.finish_and_clear();
    println!("{done}");
    return Ok(file);
}

/// Extract a 7z file with a progress bar.
fn extract_7z(
    file: File,
    password: Option<String>,
    dest: PathBuf,
    start: String,
    done: String,
) -> Result<(), Error> {
    // Initialise the 7z reader
    let len = file.metadata().map(|m| m.len()).map_err(Error::LengthDlc)?;
    let password = match password {
        Some(x) => Password::from(x.as_str()),
        None => Password::empty(),
    };
    let mut sz = sevenz_rust::SevenZReader::new(file, len, password)?;

    // Get the total size of the archive
    let archive_size: u64 = sz
        .archive()
        .files
        .iter()
        .filter(|e| e.has_stream())
        .map(|e| e.size())
        .sum();

    // Indicatif setup
    let pb = ProgressBar::new(archive_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{msg}\n{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")?
        .progress_chars("#>-"));
    pb.set_message(start);

    // Read each entry and extract it
    let mut uncompressed_size = 0;
    sz.for_each_entries(|entry, reader| {
        let mut buf = [0u8; 1024];
        let entry_name = entry.name();
        let path = dest.join(entry_name);

        // Ignore if the file exists, ignore for dlls
        if
            path.exists() &&
            path.extension().map(|x| x != "dll").unwrap_or(true)
        {
            return Ok(true);
        }

        // Check if directory
        if entry.is_directory() {
            std::fs::create_dir_all(&path)?;
            return Ok(true);
        }

        // Create the parent directory(s)
        std::fs::create_dir_all(path.parent().ok_or(sevenz_rust::Error::Other(
            Error::NoParent7z.to_string().into(),
        ))?)?;

        // Write the entry to the file
        let mut file = File::create(&path)?;
        loop {
            let read_size = reader.read(&mut buf)?;
            if read_size == 0 {
                return Ok(true);
            }
            file.write_all(&buf[..read_size])?;
            uncompressed_size += read_size;
            pb.set_position(uncompressed_size as u64);
        }
    })?;

    // Clean up and finish
    if DELETE_AFTER {
        std::fs::remove_file(ZIP_FILE)?;
    }
    pb.finish_and_clear();
    println!("{done}");
    Ok(())
}

/// Download the DLC zip.
async fn download_dlc(progress: &'static str) -> Result<File, Error> {
    // Check if the file is already present
    if PathBuf::from(ZIP_FILE).exists() {
        println!(
            "{} {} The DLC zip is already downloaded!",
            progress_style(progress),
            SPARKLE
        );
        return Ok(File::open(ZIP_FILE)?);
    }

    download_file(
        &reqwest::Client::new(),
        DLC_URL,
        &ZIP_FILE,
        format!(
            "{} {} Downloading the DLC zip...",
            progress_style(progress),
            TRUCK
        ),
        format!(
            "{} {} Downloaded the DLC zip!",
            progress_style(progress),
            SPARKLE
        ),
    )
    .await
}

/// Extract the DLC zip.
fn extract_dlc(progress: &'static str, file: File, dest: PathBuf) -> Result<(), Error> {
    extract_7z(
        file,
        PASSWORD.map(|x| x.to_string()),
        dest,
        format!(
            "{} {} Extracting the DLC zip...",
            progress_style(progress),
            CLIP
        ),
        format!(
            "{} {} Extracted the DLC zip!",
            progress_style(progress),
            SPARKLE
        ),
    )
}

/// Pause, by waiting for input.
fn pause() {
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
}

/// The main function.
async fn main_inner() -> Result<(), Error> {
    std::env::set_var("WT_SESSION", "1");

    println!(
"Welcome to the Civ6 DLC downloader!
Created by: Stefanuk12

NOTE: You need the following:
1. A stable internet connection
2. This must be ran as administrator
3. The game installed on Steam

Press enter to continue..."
    );

    pause();

    let civ6 = civ6_install("[1/4]")?;
    let dlc_zip = download_dlc("[2/4]").await?;
    extract_dlc("[3/4]", dlc_zip, civ6)?;
    println!(
        "{} {} Done, you can now run Civ6 with all of the DLC!",
        progress_style("[4/4]"),
        PAPER
    );

    pause();

    Ok(())
}

#[tokio::main]
async fn main() {
    match main_inner().await {
        Ok(_) => (),
        Err(e) => {
            eprintln!("Error: {}", e.to_string());
            pause();
        }
    }
}