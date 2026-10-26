#[path = "../terminal/mod.rs"]
mod terminal;

use clap::{Parser, Subcommand};
use gemini_file_search_mcp::api::{Answer, Gemini};
use serde_json::{Value, json};
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    name = "gfs",
    version,
    about = "Manage documents in Gemini File Search.",
    after_help = "Start without a command to open the terminal interface.\nSet GEMINI_FILE_SEARCH_API_KEY and GEMINI_UPLOAD_ROOT for API access.\nUse gfs --demo to open sample data without an API key."
)]
struct Cli {
    /// Open sample data without an API key or network access.
    #[arg(long, global = true)]
    demo: bool,
    /// Write command results as JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Select a query model. The default is GEMINI_FILE_SEARCH_MODEL or gemini-3.8-flash.
    #[arg(long, global = true)]
    model: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the terminal interface. This is the default command.
    Tui,
    /// Create, list, or delete stores.
    Stores {
        #[command(subcommand)]
        command: StoreCommand,
    },
    /// List or delete indexed documents.
    Documents {
        #[command(subcommand)]
        command: DocumentCommand,
    },
    /// Upload a local .md, .txt, or .pdf file from GEMINI_UPLOAD_ROOT.
    Upload {
        path: String,
        #[arg(long)]
        store: String,
        /// Wait for indexing. The app does not upload the file again.
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
    },
    /// Enter a question about one store. Read the answer and source citations.
    Ask {
        question: String,
        #[arg(long)]
        store: String,
    },
    /// Read the upload operation status. Use --wait to continue a wait.
    Operation {
        name: String,
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
    },
}

#[derive(Subcommand)]
enum StoreCommand {
    List {
        #[arg(long)]
        page_token: Option<String>,
    },
    Create {
        #[arg(long)]
        name: String,
    },
    /// Delete a store and all its documents.
    Delete {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum DocumentCommand {
    List {
        #[arg(long)]
        store: String,
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Delete an indexed document and its indexed content.
    Delete {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

fn api() -> Result<Gemini, String> {
    let key = std::env::var("GEMINI_FILE_SEARCH_API_KEY")
        .map_err(|_| "Set GEMINI_FILE_SEARCH_API_KEY. Use --demo to open sample data.")?;
    let root = std::env::var("GEMINI_UPLOAD_ROOT")
        .map_err(|_| "Set GEMINI_UPLOAD_ROOT to an existing directory with approved files.")?;
    Gemini::new(&key, Path::new(&root)).map_err(|error| error.to_string())
}

fn confirm(name: &str, yes: bool) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err("Use --yes to delete a resource without an interactive terminal.".into());
    }
    eprint!("To delete {}, type delete: ", terminal::clean(name));
    io::stderr()
        .flush()
        .map_err(|_| "The app cannot show the approval prompt.")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|_| "The app cannot read the approval input.")?;
    if answer.trim() == "delete" {
        Ok(())
    } else {
        Err("The app did not delete the resource.".into())
    }
}

fn print_answer(answer: &Answer) {
    println!("{}", terminal::clean(&answer.text));
    if answer.citations.is_empty() {
        println!(
            "\nGemini did not return source citations. Compare the answer with your documents."
        );
    } else {
        println!("\nSources");
        for (index, citation) in answer.citations.iter().enumerate() {
            let page = citation
                .page_number
                .map(|page| format!(" · page {page}"))
                .unwrap_or_default();
            println!(
                "[{}] {}{}\n    {}",
                index + 1,
                terminal::clean(&citation.file_name),
                page,
                terminal::clean(&citation.source)
            );
        }
    }
}

async fn run(cli: Cli) -> Result<u8, String> {
    let waited = matches!(
        cli.command,
        Some(Command::Upload { wait: true, .. } | Command::Operation { wait: true, .. })
    );
    let interactive = matches!(cli.command, None | Some(Command::Tui));
    if cli.demo && !interactive {
        return Err("Use --demo only with the terminal interface: gfs --demo".into());
    }
    if cli.json && interactive {
        return Err("Use --json with a command, for example: gfs stores list --json".into());
    }
    let model = cli
        .model
        .or_else(|| std::env::var("GEMINI_FILE_SEARCH_MODEL").ok())
        .unwrap_or_else(|| "gemini-3.8-flash".into());
    if interactive {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(
                "Use an interactive terminal for this interface. Use --help to see command options."
                    .into(),
            );
        }
        let backend = if cli.demo {
            terminal::Backend::demo()
        } else {
            terminal::Backend::Live(api()?)
        };
        terminal::run(backend, model)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(0);
    }
    let api = api()?;
    let value = match cli.command.expect("command") {
        Command::Stores {
            command: StoreCommand::List { page_token },
        } => api.list_stores(20, page_token.as_deref()).await,
        Command::Stores {
            command: StoreCommand::Create { name },
        } => api.create_store(&name).await,
        Command::Stores {
            command: StoreCommand::Delete { name, yes },
        } => {
            confirm(&name, yes)?;
            api.delete_store(&name).await
        }
        Command::Documents {
            command: DocumentCommand::List { store, page_token },
        } => api.list_documents(&store, page_token.as_deref()).await,
        Command::Documents {
            command: DocumentCommand::Delete { name, yes },
        } => {
            confirm(&name, yes)?;
            api.delete_document(&name).await
        }
        Command::Upload {
            path,
            store,
            wait,
            timeout,
        } => {
            let operation = api
                .upload_file(&store, &path)
                .await
                .map_err(|error| error.to_string())?;
            if wait {
                if !cli.json {
                    eprintln!(
                        "Indexing · {}",
                        terminal::clean(operation["name"].as_str().unwrap_or_default())
                    );
                }
                api.wait_for_operation(operation, timeout).await
            } else {
                Ok(operation)
            }
        }
        Command::Operation {
            name,
            wait,
            timeout,
        } => {
            let operation = api
                .get_operation(&name)
                .await
                .map_err(|error| error.to_string())?;
            if wait {
                api.wait_for_operation(operation, timeout).await
            } else {
                Ok(operation)
            }
        }
        Command::Ask { question, store } => {
            let answer = api
                .ask(&store, &question, &model)
                .await
                .map_err(|error| error.to_string())?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&answer)
                        .map_err(|_| "The app cannot write the answer as JSON.")?
                );
            } else {
                print_answer(&answer);
            }
            return Ok(0);
        }
        Command::Tui => unreachable!(),
    }
    .map_err(|error| error.to_string())?;
    print_value(&value, cli.json);
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return Ok(1);
    }
    if waited
        && value
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.contains("/operations/"))
        && value.get("done").and_then(Value::as_bool) != Some(true)
    {
        if !cli.json {
            eprintln!(
                "Indexing is not complete. Continue with: gfs operation <operation-name> --wait"
            );
        }
        return Ok(2);
    }
    Ok(0)
}

fn print_value(value: &Value, json_output: bool) {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
        return;
    }
    let rows = value
        .get("fileSearchStores")
        .or_else(|| value.get("documents"))
        .and_then(Value::as_array);
    if let Some(rows) = rows {
        if rows.is_empty() {
            println!("The resource list is empty.");
        }
        for row in rows {
            println!(
                "{}\n  {}  {}",
                terminal::clean(row["displayName"].as_str().unwrap_or("Resource")),
                terminal::clean(row["name"].as_str().unwrap_or_default()),
                terminal::clean(row["state"].as_str().unwrap_or_default())
            );
        }
    } else {
        println!(
            "{}",
            terminal::clean(&serde_json::to_string_pretty(value).unwrap_or_default())
        );
    }
    if let Some(token) = value.get("nextPageToken").and_then(Value::as_str) {
        println!(
            "\nMore results are available. Use --page-token with this value:\n{}",
            terminal::clean(token)
        );
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let json_errors = cli.json;
    match run(cli).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            if json_errors {
                eprintln!("{}", json!({"error": error}));
            } else {
                eprintln!("gfs: {}", terminal::clean(&error));
            }
            ExitCode::FAILURE
        }
    }
}
