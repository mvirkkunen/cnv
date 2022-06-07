use std::error::Error;
use std::ffi::OsString;
use std::io::{prelude::*, BufReader};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use structopt::StructOpt;
use x11rb::rust_connection::{DefaultStream, RustConnection};

// Guaranteed(TM) to be a unique delimiter
const DELIMITER: &str = "2920db02-ad14-4592-86be-2c9379c3ef2d";

#[derive(Debug, StructOpt)]
#[structopt(name = "cnv", about = "Convenient Network Viewer")]
struct Opt {
    /// X11 magic cookie
    #[structopt(long)]
    cookie: Option<String>,

    /// SSH options (Name=value)
    #[structopt(short = "o", number_of_values = 1)]
    option: Vec<String>,

    /// SSH port
    #[structopt(short = "p")]
    port: Option<u16>,

    /// SSH proxy jump addresses
    #[structopt(short = "J", number_of_values = 1)]
    jump: Vec<String>,

    /// SSH host
    #[structopt(name = "HOST")]
    host: String,

    #[structopt(flatten)]
    cnv_config: cnv::Config,
}

fn main() -> Result<(), Box<dyn Error>> {
    let opt = Opt::from_args();

    let temp_dir = tempfile::Builder::new().prefix("cnv").tempdir()?;

    let conn_path1 = temp_dir.path().join("conn1");
    let conn_path2 = temp_dir.path().join("conn2");

    let mut arg1 = OsString::new();
    arg1.push(&conn_path1);
    arg1.push(OsString::from(":".to_string()));
    arg1.push("/tmp/.X11-unix/X0");

    let mut arg2 = OsString::new();
    arg2.push(&conn_path2);
    arg2.push(OsString::from(":".to_string()));
    arg2.push("/tmp/.X11-unix/X0");

    let mut cmd = Command::new("ssh");

    cmd.arg("-f")
        .arg("-C")
        .arg("-L")
        .arg(arg1.clone())
        .arg("-L")
        .arg(arg2.clone())
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    if let Some(port) = opt.port {
        cmd.arg("-p").arg(port.to_string());
    }

    for jump in opt.jump {
        cmd.arg("-J").arg(jump);
    }

    for option in opt.option {
        cmd.arg("-o").arg(option);
    }

    cmd.arg(opt.host).arg(format!(
        "cat .Xauthority || echo -n; echo {}; sleep infinity",
        DELIMITER
    ));

    let cmd = cmd.spawn()?;

    let mut output = BufReader::new(cmd.stdout.unwrap());
    let mut buf = Vec::new();

    let end = loop {
        output.read_until(b'\n', &mut buf)?;

        if let Some(e) = buf
            .windows(DELIMITER.len())
            .position(|w| w == DELIMITER.as_bytes())
        {
            break e;
        }
    };

    println!("SSH connected");

    let entries = cnv::xauth::parse(&mut &buf[..end])?;

    let mut auth = match entries.iter().find(|e| e.family == 256) {
        Some(e) => e.clone(),
        None => cnv::xauth::Entry {
            family: 256,
            address: Vec::new(),
            number: Vec::new(),
            name: Vec::new(),
            data: Vec::new(),
        },
    };

    if let Some(cookie) = opt.cookie {
        auth.name = "MIT-MAGIC-COOKIE-1".as_bytes().to_vec();
        auth.data = cookie.as_bytes().to_vec();
    }

    let rconn = connection_unix(conn_path1, opt.cnv_config.screen, &auth)?;
    let rconn2 = connection_unix(conn_path2, opt.cnv_config.screen, &auth)?;

    println!("X11 connected");

    cnv::run(&opt.cnv_config, rconn, rconn2)?;

    Ok(())
}

fn connection_unix(
    path: impl AsRef<Path>,
    screen: usize,
    auth: &cnv::xauth::Entry,
) -> Result<RustConnection, Box<dyn Error>> {
    let rstream = DefaultStream::from_unix_stream(UnixStream::connect(path)?)?;

    let rconn = RustConnection::connect_to_stream_with_auth_info(
        rstream,
        screen,
        auth.name.clone(),
        auth.data.clone(),
    )?;

    Ok(rconn)
}
