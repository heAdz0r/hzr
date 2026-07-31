use std::ffi::{OsStr, OsString};
use std::path::Path;

pub fn normalize(args: Vec<OsString>) -> Vec<OsString> {
    if args.first().and_then(|arg| Path::new(arg).file_name()) != Some(OsStr::new("rtk")) {
        return args;
    }

    let mut normalized = Vec::with_capacity(args.len() + 2);
    normalized.extend([
        OsString::from("hzr"),
        OsString::from("rtk"),
        OsString::from("--"),
    ]);
    normalized.extend(args.into_iter().skip(1));
    normalized
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use clap::Parser;

    use super::normalize;
    use crate::cli::{Cli, Command};

    #[test]
    fn test_normalize_rewrites_installed_rtk_alias() {
        let normalized = normalize(
            ["/usr/local/bin/rtk", "rgai", "--json", "authentication"]
                .into_iter()
                .map(OsString::from)
                .collect(),
        );

        assert_eq!(
            normalized,
            ["hzr", "rtk", "--", "rgai", "--json", "authentication"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
        let cli = Cli::try_parse_from(normalized).expect("normalized compatibility command");
        assert!(matches!(&cli.command, Command::Rtk(_)));
        if let Command::Rtk(arguments) = cli.command {
            assert_eq!(
                arguments.args,
                ["rgai", "--json", "authentication"]
                    .into_iter()
                    .map(OsString::from)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn test_normalize_leaves_hzr_arguments_unchanged() {
        let arguments = ["hzr", "rtk", "--", "read", "Cargo.toml"]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();

        assert_eq!(normalize(arguments.clone()), arguments);
    }

    #[cfg(unix)]
    #[test]
    fn test_normalize_preserves_non_utf8_fork_argument() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let opaque = OsString::from_vec(vec![b'f', 0x80, b'o']);
        let normalized = normalize(vec![
            OsString::from("rtk"),
            OsString::from("read"),
            opaque.clone(),
        ]);
        let cli = Cli::try_parse_from(normalized).expect("non-UTF-8 fork argument parses");
        assert!(matches!(&cli.command, Command::Rtk(_)));
        if let Command::Rtk(arguments) = cli.command {
            assert_eq!(arguments.args[1].as_bytes(), opaque.as_bytes());
        }
    }
}
