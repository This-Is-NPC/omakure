use std::error::Error;

pub struct CompletionOptions {
    pub shell: String,
}

pub fn print_completion_help() {
    println!(
        "Usage: omakure completion <shell>\n\n\
Supported shells:\n\
  bash | zsh | fish | pwsh"
    );
}

pub fn parse_completion_args(args: &[String]) -> Result<CompletionOptions, Box<dyn Error>> {
    if args.is_empty() {
        return Err("Missing shell name. Use `omakure completion <shell>`.".into());
    }
    if args.len() > 1 {
        return Err("completion expects a single shell argument".into());
    }

    Ok(CompletionOptions {
        shell: args[0].to_string(),
    })
}

pub fn run_completion(options: CompletionOptions) -> Result<(), Box<dyn Error>> {
    let shell = options.shell.as_str();
    match shell {
        "bash" => {
            println!("{}", bash_completion());
        }
        "zsh" => {
            println!("{}", zsh_completion());
        }
        "fish" => {
            println!("{}", fish_completion());
        }
        "pwsh" | "powershell" => {
            println!("{}", pwsh_completion());
        }
        _ => {
            return Err(format!("Unsupported shell: {}", shell).into());
        }
    }

    Ok(())
}

fn bash_completion() -> &'static str {
    r#"_omakure_complete() {
  local cur prev
  cur="${COMP_WORDS[COMP_CWORD]}"
  prev="${COMP_WORDS[COMP_CWORD-1]}"

  local commands="update uninstall doctor check list install scripts run init config env completion help version"

  if [[ ${COMP_CWORD} -eq 1 ]]; then
    COMPREPLY=( $(compgen -W "${commands}" -- "${cur}") )
    return 0
  fi

  case "${prev}" in
    update)
      COMPREPLY=( $(compgen -W "--repo --version" -- "${cur}") )
      return 0
      ;;
    uninstall)
      COMPREPLY=( $(compgen -W "--scripts" -- "${cur}") )
      return 0
      ;;
    install)
      COMPREPLY=( $(compgen -W "--name" -- "${cur}") )
      return 0
      ;;
    completion)
      COMPREPLY=( $(compgen -W "bash zsh fish pwsh" -- "${cur}") )
      return 0
      ;;
  esac
}

complete -F _omakure_complete omakure
"#
}

fn zsh_completion() -> &'static str {
    r#"#compdef omakure

_omakure() {
  local -a commands
  commands=(
    'update:Update omakure from GitHub Releases'
    'uninstall:Remove the omakure binary'
    'doctor:Check runtime dependencies and workspace'
    'check:Alias for doctor'
    'list:List Omaken flavors'
    'install:Install an Omaken flavor'
    'scripts:List available scripts'
    'run:Run a script without the TUI'
    'init:Create a new script template'
    'config:Show resolved paths and env'
    'env:Alias for config'
    'completion:Generate shell completion'
    'help:Show help'
    'version:Show version'
  )

  _arguments \
    '1:command:->command' \
    '*::arg:->args'

  case $state in
    command)
      _describe -t commands 'omakure commands' commands
      ;;
    args)
      case $words[2] in
        update)
          _arguments '--repo[GitHub repository]' '--version[Release tag]'
          ;;
        uninstall)
          _arguments '--scripts[Remove scripts directory]'
          ;;
        install)
          _arguments '--name[Override the target folder name]'
          ;;
        completion)
          _arguments '1:shell:(bash zsh fish pwsh)'
          ;;
      esac
      ;;
  esac
}

_omakure "$@"
"#
}

fn fish_completion() -> &'static str {
    r#"complete -c omakure -f -a "update uninstall doctor check list install scripts run init config env completion help version"
complete -c omakure -n '__fish_seen_subcommand_from update' -l repo -d "GitHub repository"
complete -c omakure -n '__fish_seen_subcommand_from update' -l version -d "Release tag"
complete -c omakure -n '__fish_seen_subcommand_from uninstall' -l scripts -d "Remove scripts directory"
complete -c omakure -n '__fish_seen_subcommand_from install' -l name -d "Override the target folder name"
complete -c omakure -n '__fish_seen_subcommand_from completion' -f -a "bash zsh fish pwsh"
"#
}

fn pwsh_completion() -> &'static str {
    r#"Register-ArgumentCompleter -Native -CommandName omakure -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $commands = @('update','uninstall','doctor','check','list','install','scripts','run','init','config','env','completion','help','version')
    $elements = $commandAst.CommandElements
    if ($elements.Count -le 2) {
        $commands | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
            [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
        }
        return
    }

    $sub = $elements[1].Value
    $options = @()
    switch ($sub) {
        'update' { $options = @('--repo','--version') }
        'uninstall' { $options = @('--scripts') }
        'install' { $options = @('--name') }
        'completion' { $options = @('bash','zsh','fish','pwsh') }
    }

    $options | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
    }
}
"#
}
