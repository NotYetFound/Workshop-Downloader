use std::{ffi::OsString, path::Path};

pub const CHILD_FLAG: &str = "--workshop-backend-child";
pub const START_ERROR: &str = "Workshop Downloader could not start backend write isolation.";
const DOTNET_IPC: &str = "/tmp/.dotnet/shm";

// .NET 9 uses this fixed directory for named mutexes, independently of TMPDIR.
pub(super) fn prepare_dotnet_ipc() -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags, fchmod, open};
    use std::{fs, io, os::unix::fs::DirBuilderExt};
    for (path, mode) in [("/tmp/.dotnet", 0o777), (DOTNET_IPC, 0o1777)] {
        match fs::DirBuilder::new().mode(mode).create(path) {
            Ok(()) => {
                let directory = open(
                    path,
                    OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                fchmod(&directory, Mode::from_raw_mode(mode))?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                // Never follow an existing symlink in the shared temporary tree.
                let _ = open(
                    path,
                    OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub fn exec(arguments: impl Iterator<Item = OsString>) -> Result<(), Box<dyn std::error::Error>> {
    use landlock::{
        ABI, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr,
    };
    use std::{io, os::unix::process::CommandExt, process::Command};

    let mut arguments = arguments.peekable();
    let dotnet = arguments.peek().is_some_and(|arg| arg == "--dotnet-ipc");
    if dotnet {
        arguments.next();
    }
    let mut writable = Vec::new();
    while arguments.peek().is_some_and(|arg| arg != "--") {
        let directory = arguments.next().expect("argument was checked");
        if !Path::new(&directory).is_absolute() {
            return Err(io::Error::other("expected an absolute workspace").into());
        }
        writable.push(directory);
    }
    if arguments.next().is_none() || writable.is_empty() {
        return Err(io::Error::other("backend workspace and command are required").into());
    }
    let executable = arguments
        .next()
        .ok_or_else(|| io::Error::other("missing backend executable"))?;
    if !Path::new(&executable).is_absolute() {
        return Err(io::Error::other("expected an absolute backend executable").into());
    }

    // Scope writes without blocking system libraries, certificate reads, or Steam networking.
    // This helper runs before the GUI initializes, so the policy covers every backend thread.
    let writes = AccessFs::from_write(ABI::V3);
    let mut rules = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(writes)?
        .create()?;
    for directory in writable {
        rules = rules.add_rule(PathBeneath::new(PathFd::new(directory)?, writes))?;
    }
    if dotnet {
        let mutex = PathFd::new(DOTNET_IPC)?;
        let access = AccessFs::WriteFile
            | AccessFs::Truncate
            | AccessFs::MakeDir
            | AccessFs::MakeReg
            | AccessFs::RemoveDir
            | AccessFs::RemoveFile;
        rules = rules.add_rule(PathBeneath::new(mutex, access))?;
    }
    for device in ["/dev/null", "/dev/tty"] {
        rules = rules.add_rule(PathBeneath::new(
            PathFd::new(device)?,
            AccessFs::WriteFile | AccessFs::Truncate,
        ))?;
    }
    rules.restrict_self()?;
    Err(Command::new(executable).args(arguments).exec().into())
}
