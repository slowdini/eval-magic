use super::*;

use super::super::{PACKAGE_REASON, classify_mutation_targets};

const ROOTS: [&str; 1] = ["/work/env"];

/// Every case runs through the whole classifier, not just this module, so the
/// segment splitting and the older recognizers stay part of what is asserted.
fn classify(command: &str) -> Option<BashClassification> {
    classify_mutation_targets(command, &ROOTS.map(str::to_string), Path::new("/work/env"))
}

/// The case from #307: a direct shell mutation aimed outside the task
/// environment is contained, and the denial names the path it resolved.
#[test]
fn touch_outside_an_allowed_root_is_denied_with_its_resolved_target() {
    let denial = classify("touch /private/tmp/probe").expect("out-of-root touch should deny");
    assert_eq!(denial.reason, "file creation (touch)");
    assert_eq!(
        denial.resolved_targets,
        vec!["/private/tmp/probe".to_string()]
    );
}

#[test]
fn touch_inside_an_allowed_root_is_allowed() {
    for command in [
        "touch out.txt",
        "touch ./nested/out.txt",
        "touch /work/env/out.txt",
        "touch a.txt b.txt",
        "touch -a -m out.txt",
        "touch -- out.txt",
        "touch -- -weird-name",
        "touch -r template.txt out.txt",
        "touch --reference=template.txt out.txt",
        "touch -d 'last thursday' out.txt",
        "touch -t 202401010000 out.txt",
    ] {
        assert_eq!(classify(command), None, "{command}");
    }
}

/// An option operand is not a mutation target, so a `touch` whose only
/// out-of-root word is the operand of `-r`, `-d`, or `-t` stays allowed —
/// reading a reference file outside the root is not a write.
#[test]
fn touch_option_operands_are_not_mutation_targets() {
    for command in [
        "touch -r /etc/passwd out.txt",
        "touch --reference /etc/passwd out.txt",
        "touch -t 202401010000 out.txt",
    ] {
        assert_eq!(classify(command), None, "{command}");
    }
}

#[test]
fn touch_denies_every_out_of_root_spelling() {
    for command in [
        "touch /private/tmp/probe",
        "touch ../escape.txt",
        "touch out.txt /private/tmp/probe",
        "touch -- /private/tmp/probe",
        "touch -a /private/tmp/probe",
    ] {
        assert_eq!(
            classify(command).map(|denial| denial.reason),
            Some(TOUCH_REASON),
            "{command}"
        );
    }
}

/// Compound commands are already split per segment upstream; a mutation in any
/// segment is still contained.
#[test]
fn touch_is_contained_in_every_segment_of_a_compound_command() {
    for command in [
        "cd /work/env && touch /private/tmp/probe",
        "touch /private/tmp/probe; echo done",
        "echo hi | touch /private/tmp/probe",
    ] {
        assert_eq!(
            classify(command).map(|denial| denial.reason),
            Some(TOUCH_REASON),
            "{command}"
        );
    }
}

/// The classifier anchors on the executable, so a mutator name appearing as an
/// argument to something else is not a mutation.
#[test]
fn a_mutator_name_used_as_an_argument_is_not_a_mutation() {
    for command in [
        "echo touch /private/tmp/probe",
        "grep touch /private/tmp/probe",
        "git commit -m touch",
    ] {
        assert_eq!(classify(command), None, "{command}");
    }
}

/// Wrappers keep the mutation visible: the executable is the wrapped command.
#[test]
fn touch_through_a_wrapper_is_still_contained() {
    for command in [
        "sudo touch /private/tmp/probe",
        "env touch /private/tmp/probe",
        "env FOO=bar touch /private/tmp/probe",
        "FOO=bar touch /private/tmp/probe",
        "command touch /private/tmp/probe",
        "nohup touch /private/tmp/probe",
        "timeout 5 touch /private/tmp/probe",
        "nice -n 5 touch /private/tmp/probe",
        "/usr/bin/touch /private/tmp/probe",
    ] {
        assert_eq!(
            classify(command).map(|denial| denial.reason),
            Some(TOUCH_REASON),
            "{command}"
        );
    }
}

#[test]
fn mkdir_is_contained_by_its_directory_operands() {
    for command in [
        "mkdir build",
        "mkdir -p src/generated",
        "mkdir -m 700 secrets",
        "mkdir -m700 secrets",
        "mkdir --mode=700 secrets",
        "mkdir -p a b c",
        "mkdir -- -p",
    ] {
        assert_eq!(classify(command), None, "{command}");
    }
    for command in [
        "mkdir /private/tmp/probe",
        "mkdir -p /private/tmp/probe",
        "mkdir -m 700 /private/tmp/probe",
        "mkdir build /private/tmp/probe",
        "mkdir ../escape",
        "sudo mkdir -p /etc/probe",
    ] {
        assert_eq!(
            classify(command).map(|denial| denial.reason),
            Some(MKDIR_REASON),
            "{command}"
        );
    }
}

/// `mkdir -m MODE` takes its mode in the following word; the mode is not a
/// directory and must not be read as one.
#[test]
fn mkdir_mode_operand_is_not_a_directory_operand() {
    assert_eq!(classify("mkdir -m 700 build"), None);
}

#[test]
fn rm_is_contained_by_everything_it_deletes() {
    for command in [
        "rm out.txt",
        "rm -f out.txt",
        "rm -rf target",
        "rm -r -f target",
        "rm --recursive --force target",
        "rm a.txt b.txt",
        "rm -- -weird-name",
    ] {
        assert_eq!(classify(command), None, "{command}");
    }
    for command in [
        "rm -rf /",
        "rm /etc/passwd",
        "rm -rf /private/tmp/probe",
        "rm out.txt /etc/passwd",
        "rm -rf ../escape",
        "sudo rm -rf /etc",
    ] {
        assert_eq!(
            classify(command).map(|denial| denial.reason),
            Some(RM_REASON),
            "{command}"
        );
    }
}

#[test]
fn rm_records_the_path_it_would_delete() {
    let denial = classify("rm -rf /private/tmp/probe").expect("out-of-root rm should deny");
    assert_eq!(
        denial.resolved_targets,
        vec!["/private/tmp/probe".to_string()]
    );
}

#[test]
fn cp_is_contained_by_its_destination_and_not_by_its_sources() {
    for command in [
        "cp a.txt b.txt",
        "cp -r src dest",
        "cp a.txt b.txt dest/",
        // Reading a file outside the root is not a write; only the
        // destination is a mutation target.
        "cp /etc/passwd ./copy",
        "cp -r /etc ./etc-backup",
        "cp -t dest a.txt b.txt",
        "cp --target-directory=dest a.txt",
        "cp -S .bak a.txt b.txt",
        "cp --suffix=.bak a.txt b.txt",
        "cp --sparse always a.txt b.txt",
        // `--backup` takes an optional argument, so it never consumes the
        // following word; `a.txt` stays a source and `b.txt` the destination.
        "cp --backup a.txt b.txt",
    ] {
        assert_eq!(classify(command), None, "{command}");
    }
    for command in [
        "cp a.txt /private/tmp/probe",
        "cp -r src /private/tmp/probe",
        "cp a.txt b.txt /private/tmp/",
        "cp -t /private/tmp a.txt",
        "cp --target-directory=/private/tmp a.txt",
        "cp --backup a.txt /private/tmp/probe",
        "cp a.txt ../escape",
    ] {
        assert_eq!(
            classify(command).map(|denial| denial.reason),
            Some(CP_REASON),
            "{command}"
        );
    }
}

/// A short option with a required argument takes the rest of its cluster as
/// that argument, or the following word when the cluster ends there — the same
/// rule getopt applies.
#[test]
fn cp_reads_destination_options_out_of_bundled_short_clusters() {
    // `-rt DIR` — the cluster ends at `t`, so the directory is the next word.
    assert_eq!(
        classify("cp -rt /private/tmp src").map(|denial| denial.reason),
        Some(CP_REASON)
    );
    assert_eq!(classify("cp -rt dest src"), None);
    // `-tr` — `t` takes the rest of the cluster, so the destination is `r`.
    assert_eq!(classify("cp -tr /private/tmp/src"), None);
    // A mode-style cluster must not swallow the destination.
    assert_eq!(classify("cp -rS .bak a.txt b.txt"), None);
}

#[test]
fn cp_records_the_destination_it_would_write() {
    let denial = classify("cp -r src /private/tmp/probe").expect("out-of-root cp should deny");
    assert_eq!(
        denial.resolved_targets,
        vec!["/private/tmp/probe".to_string()]
    );
}

/// `mv` removes what it moves, so an out-of-root *source* is a mutation too —
/// unlike `cp`, where the source is only read.
#[test]
fn mv_is_contained_by_both_its_destination_and_its_sources() {
    for command in [
        "mv a.txt b.txt",
        "mv a.txt b.txt dest/",
        "mv -t dest a.txt b.txt",
        "mv --target-directory=dest a.txt",
        "mv --suffix=.bak a.txt b.txt",
        "mv -f a.txt b.txt",
    ] {
        assert_eq!(classify(command), None, "{command}");
    }
    for command in [
        "mv a.txt /private/tmp/probe",
        "mv /etc/hosts ./captured",
        "mv -t /private/tmp a.txt",
        "mv -t dest /etc/hosts",
        "mv a.txt ../escape",
    ] {
        assert_eq!(
            classify(command).map(|denial| denial.reason),
            Some(MV_REASON),
            "{command}"
        );
    }
}

#[test]
fn install_is_contained_by_its_destination() {
    for command in [
        "install src dest",
        "install -m 755 src dest",
        "install -m755 src dest",
        "install --mode=755 src dest",
        "install -Dm 755 src dest",
        "install -o root -g root src dest",
        "install --strip-program=strip src dest",
        "install -t dest src",
        "install -d build/generated",
        "install --directory build/generated",
        "install -d a b c",
        // The source is only read.
        "install /etc/hosts ./hosts",
    ] {
        assert_eq!(classify(command), None, "{command}");
    }
    for command in [
        "install src /private/tmp/probe",
        "install -m 755 src /private/tmp/probe",
        "install -t /private/tmp src",
        "install -d /private/tmp/probe",
        "install --directory /private/tmp/probe",
        "install -d build /private/tmp/probe",
        "install -Dm 755 src /private/tmp/probe",
    ] {
        assert_eq!(
            classify(command).map(|denial| denial.reason),
            Some(INSTALL_REASON),
            "{command}"
        );
    }
}

/// `install` is also the action word of every package manager. Anchoring on the
/// executable keeps the two apart, so the package recognizers keep their own
/// reasons and an in-root package install stays allowed.
#[test]
fn a_package_manager_install_is_not_a_coreutils_install() {
    assert_eq!(classify("npm install left-pad"), None);
    assert_eq!(classify("pip install -r requirements.txt"), None);
    assert_eq!(
        classify_mutation_targets(
            "npm install left-pad --prefix /private/tmp",
            &ROOTS.map(str::to_string),
            Path::new("/work/env"),
        )
        .map(|denial| denial.reason),
        Some(PACKAGE_REASON)
    );
}

/// A glob or brace expands to entries of the directory its literal prefix
/// names, so a single-component expansion under an in-root directory cannot
/// reach outside it and stays allowed.
#[test]
fn a_glob_confined_to_an_in_root_directory_is_allowed() {
    for command in [
        "rm -rf target/*",
        "rm *.log",
        "rm -f build/*.o",
        "mkdir -p build/{debug,release}",
        "touch build/*.stamp",
        "cp -r src/* dest",
        "mv build/*.tar dist",
    ] {
        assert_eq!(classify(command), None, "{command}");
    }
}

/// Everything else dynamic still fails closed: a variable or command expansion
/// names no directory, a multi-component glob can walk upward, and `.*` matches
/// `..`.
#[test]
fn an_unconfined_expansion_is_still_denied() {
    for command in [
        "rm -rf $BUILD",
        "rm -rf \"$BUILD/x\"",
        "rm -rf ~/cache",
        "rm -rf `cat targets`",
        "rm -rf $(cat targets)",
        "rm -rf .*",
        "rm -rf target/*/*",
        "rm -rf target/*/../../..",
    ] {
        assert_eq!(
            classify(command).map(|denial| denial.reason),
            Some(RM_REASON),
            "{command}"
        );
    }
    assert_eq!(
        classify("touch $OUT").map(|denial| denial.reason),
        Some(TOUCH_REASON)
    );
    // The denial names no path: a half-expanded spelling is not evidence.
    let denial = classify("rm -rf $BUILD").expect("an unresolved target should deny");
    assert!(denial.resolved_targets.is_empty(), "{denial:?}");
}

/// A glob whose literal directory sits outside the root is denied, and the
/// denial names that directory rather than an unexpanded pattern.
#[test]
fn a_glob_under_an_out_of_root_directory_is_denied_with_that_directory() {
    let denial = classify("rm -rf /private/tmp/*").expect("out-of-root glob should deny");
    assert_eq!(denial.reason, RM_REASON);
    assert_eq!(denial.resolved_targets, vec!["/private/tmp".to_string()]);
}
