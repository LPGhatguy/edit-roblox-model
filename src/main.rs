#![windows_subsystem = "windows"]

use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use anyhow::bail;
use clap::Parser;
use notify::{DebouncedEvent, RecursiveMode, Watcher};
use rbx_dom_weak::{InstanceBuilder, WeakDom};
use roblox_install::RobloxStudio;

#[derive(Parser)]
struct Args {
    path: PathBuf,
}

fn run() -> anyhow::Result<()> {
    let args = Args::parse();

    let ext = args.path.extension().and_then(|ext| ext.to_str());
    let file = BufReader::new(fs_err::File::open(&args.path)?);
    let mut model_dom = match ext {
        Some("rbxm") => rbx_binary::from_reader(file)?,
        Some("rbxmx") => rbx_xml::from_reader_default(file)?,
        _ => bail!("Unknown file type '{}'", ext.unwrap_or("(no extension)")),
    };

    let dir = tempfile::tempdir()?;
    let place_path = dir.path().join("edit-roblox-model temp place.rbxl");
    let mut place_dom = WeakDom::new(InstanceBuilder::new("DataModel"));
    let workspace = place_dom.insert(place_dom.root_ref(), InstanceBuilder::new("Workspace"));

    let objects = model_dom.root().children().to_owned();
    for object in objects {
        model_dom.transfer(object, &mut place_dom, workspace);
    }

    let place_file = BufWriter::new(fs_err::File::create(&place_path)?);
    rbx_binary::to_writer(place_file, &place_dom, place_dom.root().children())?;

    let (tx, rx) = channel();
    let mut watcher = notify::watcher(tx, Duration::from_millis(200))?;
    watcher.watch(&place_path, RecursiveMode::NonRecursive)?;

    {
        let place_path = place_path.clone();
        let model_path = args.path.clone();

        let rebuild = move || {
            let place_file = BufReader::new(fs_err::File::open(&place_path)?);
            let mut place_dom = rbx_binary::from_reader(place_file)?;

            let workspace = place_dom
                .root()
                .children()
                .iter()
                .find_map(|&referent| {
                    let object = place_dom.get_by_ref(referent).unwrap();
                    if object.name == "Workspace" {
                        Some(object)
                    } else {
                        None
                    }
                })
                .unwrap();

            let workspace_children = workspace.children().to_owned();

            let mut model = WeakDom::new(InstanceBuilder::new("DataModel"));
            let model_root = model.root_ref();
            for object_ref in workspace_children {
                let object = place_dom.get_by_ref(object_ref).unwrap();

                match (object.class.as_str(), object.name.as_str()) {
                    ("Camera", "Camera") => continue,
                    ("Terrain", "Terrain") => continue,
                    _ => (),
                }

                place_dom.transfer(object_ref, &mut model, model_root);
            }

            let model_file = BufWriter::new(fs_err::File::create(&model_path)?);
            rbx_binary::to_writer(model_file, &model, model.root().children())?;

            Result::<(), anyhow::Error>::Ok(())
        };

        thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                if let DebouncedEvent::Write(_) = event {
                    println!("Saving model...");

                    if let Err(err) = rebuild() {
                        eprintln!("Error processing place file: {err:?}");
                    }
                }
            }
        });
    }

    let studio = RobloxStudio::locate()?;
    let status = Command::new(studio.application_path())
        .arg(&place_path)
        .status()?;

    if !status.success() {
        bail!("Roblox Studio exited with an error.");
    }

    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("Error: {err:?}");
        let _ = msgbox::create("Error", &format!("{err:?}"), msgbox::IconType::Error);

        std::process::exit(1);
    }
}
