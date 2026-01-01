use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct UninstallOptions {
    pub scripts_dir: PathBuf,
    pub remove_scripts: bool,
}

pub fn print_uninstall_help() {
    println!(
        "Usage: omakure uninstall [--scripts]\n\n\
Options:\n\
  --scripts   Remove the scripts directory as well\n\n\
Environment:\n\
  OMAKURE_SCRIPTS_DIR  Scripts directory override\n\
  OVERTURE_SCRIPTS_DIR  Legacy scripts directory override\n\
  CLOUD_MGMT_SCRIPTS_DIR  Legacy scripts directory override"
    );
}

pub fn parse_uninstall_args(
    args: &[String],
    scripts_dir: PathBuf,
) -> Result<UninstallOptions, Box<dyn Error>> {
    let mut options = UninstallOptions {
        scripts_dir,
        remove_scripts: false,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--scripts" => {
                options.remove_scripts = true;
                i += 1;
            }
            unknown => {
                return Err(format!("Unknown uninstall arg: {}", unknown).into());
            }
        }
    }

    Ok(options)
}

pub fn run_uninstall(options: UninstallOptions) -> Result<(), Box<dyn Error>> {
    let exe = env::current_exe()?;

    if cfg!(windows) {
        uninstall_windows(&exe)?;
    } else {
        uninstall_unix(&exe)?;
    }

    if options.remove_scripts {
        if options.scripts_dir.exists() {
            std::fs::remove_dir_all(&options.scripts_dir)?;
            println!("Removed scripts folder: {}", options.scripts_dir.display());
        } else {
            println!("Scripts folder not found: {}", options.scripts_dir.display());
        }
    }

    Ok(())
}

fn uninstall_unix(exe: &Path) -> Result<(), Box<dyn Error>> {
    match std::fs::remove_file(exe) {
        Ok(()) => println!("Removed {}", exe.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!("Binary already removed: {}", exe.display())
        }
        Err(err) => return Err(err.into()),
    }

    Ok(())
}

fn uninstall_windows(exe: &Path) -> Result<(), Box<dyn Error>> {
    let script = format!(
        "$processId = {pid}; \
         try {{ $p = Get-Process -Id $processId -ErrorAction SilentlyContinue; if ($p) {{ $p.WaitForExit(); }} }} catch {{}}; \
         $target = {target}; \
         if (Test-Path -LiteralPath $target) {{ Remove-Item -LiteralPath $target -Force; }} \
         $installDir = Split-Path -Parent $target; \
         if (Test-Path -LiteralPath $installDir) {{ \
           $items = Get-ChildItem -LiteralPath $installDir -Force -ErrorAction SilentlyContinue; \
           if (-not $items) {{ Remove-Item -LiteralPath $installDir -Force; }} \
         }} \
         $rootDir = Split-Path -Parent $installDir; \
         if (Test-Path -LiteralPath $rootDir) {{ \
           $items = Get-ChildItem -LiteralPath $rootDir -Force -ErrorAction SilentlyContinue; \
           if (-not $items) {{ Remove-Item -LiteralPath $rootDir -Force; }} \
         }} \
         function Normalize-Path([string]$p) {{ return ($p.Trim('\"').TrimEnd('\\')).ToLowerInvariant(); }} \
         $envKey = 'HKCU:\\Environment'; \
         try {{ $pathValue = (Get-ItemProperty -Path $envKey -Name Path -ErrorAction SilentlyContinue).Path }} catch {{ $pathValue = $null }}; \
         if ($pathValue) {{ \
           $normalizedInstall = Normalize-Path $installDir; \
           $parts = $pathValue -split ';' | Where-Object {{ $_ -and (Normalize-Path $_) -ne $normalizedInstall }}; \
           $newPath = ($parts -join ';'); \
           if ($newPath -ne $pathValue) {{ Set-ItemProperty -Path $envKey -Name Path -Value $newPath; }} \
         }}",
        pid = std::process::id(),
        target = ps_quote(&exe.display().to_string())
    );

    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .spawn()?;

    println!("Uninstall will finish after this process exits.");
    Ok(())
}

fn ps_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "''"))
}
