use super::docker_config::DockerProjectConfig;
use super::docker_size::*;
use super::project_cloud::DevcontainerInfo;

fn asks(cpus: Option<u32>, gib: Option<u64>) -> DevcontainerInfo {
    DevcontainerInfo {
        host_cpus: cpus,
        host_memory_mib: gib.map(|g| g * 1024),
        ..Default::default()
    }
}

fn kept(fit: &SizeFit) -> (u32, &str) {
    (fit.cpus, fit.memory.as_str())
}

#[test]
fn a_saved_shape_that_meets_the_devcontainer_is_kept() {
    let none = DevcontainerInfo::default();
    assert_eq!(fit_size(2, "4g", &none).note, None);
    // A bigger one than asked for stays too: never lowered.
    let fit = fit_size(8, "16g", &asks(Some(4), Some(8)));
    assert_eq!(kept(&fit), (8, "16g"));
    assert_eq!(fit.note, None);
    // Case and spaces of the saved text do not matter.
    assert_eq!(kept(&fit_size(2, " 4G ", &none)), (2, "4g"));
}

#[test]
fn a_saved_size_below_the_devcontainer_is_raised_with_a_note() {
    // The saved default under a 4 CPU / 8 GB devcontainer.
    let fit = fit_size(2, "4g", &asks(Some(4), Some(8)));
    assert_eq!(kept(&fit), (4, "8g"));
    assert_eq!(
        fit.note.as_deref(),
        Some("The devcontainer asks for 4 CPUs and 8 GB; raised 2 CPU / 4g to 4 CPU / 8g for this start.")
    );
    // Memory alone can need the bigger shape, CPUs alone as well.
    assert_eq!(kept(&fit_size(2, "4g", &asks(None, Some(6)))), (4, "8g"));
    assert_eq!(kept(&fit_size(2, "4g", &asks(Some(8), None))), (8, "16g"));
}

#[test]
fn a_requirement_beyond_the_cap_keeps_the_size_and_says_so() {
    // 16 CPUs, 32 GB or more than any shape: the raise stops at the cap (8 CPU / 16g) and a
    // requirement above it is never billed unasked.
    for info in [
        asks(Some(16), None),
        asks(Some(32), None),
        asks(None, Some(32)),
        asks(Some(4), Some(64)),
    ] {
        let fit = fit_size(2, "4g", &info);
        assert_eq!(kept(&fit), (2, "4g"), "{info:?}");
        let note = fit.note.unwrap();
        assert!(note.contains("more than a start raises a size to by itself (8 CPU / 16g)"));
        assert!(note.contains("using 2 CPU / 4g"), "{note}");
    }
    // Exactly the cap is still raised to.
    assert_eq!(
        kept(&fit_size(2, "4g", &asks(Some(8), Some(16)))),
        (8, "16g")
    );
    // A saved size already above the cap is left alone.
    assert_eq!(
        kept(&fit_size(16, "32g", &asks(Some(32), None))),
        (16, "32g")
    );
}

#[test]
fn a_saved_size_that_is_not_a_shape_is_rounded_up_never_down() {
    for (cpus, memory, shape) in [
        (8, "8g", (8, "16g")),
        (4, "4g", (4, "8g")),
        (3, "4g", (4, "8g")),
        (1, "6g", (4, "8g")),
        (2, "512m", (2, "4g")),
        (32, "64g", (16, "32g")),
    ] {
        let fit = fit_size(cpus, memory, &DevcontainerInfo::default());
        assert_eq!(kept(&fit), shape, "{cpus} CPU / {memory}");
        let note = fit.note.unwrap();
        assert!(
            note.contains("not a size Docker Sandboxes offers"),
            "{note}"
        );
    }
    // Then raised for the devcontainer when still short, both notes given.
    let fit = fit_size(2, "3g", &asks(Some(4), None));
    assert_eq!(kept(&fit), (4, "8g"));
    let note = fit.note.unwrap();
    assert!(note.contains("2 CPU / 3g is not a size"), "{note}");
    assert!(note.contains("The devcontainer asks for 4 CPUs"), "{note}");
}

#[test]
fn a_lone_cpus_or_memory_is_paired_with_its_shape() {
    let saved = DockerProjectConfig::default();
    let none = DevcontainerInfo::default();
    let size = |cpus, memory| requested_size(&saved, cpus, memory, &none);
    // `cpus: 4` alone used to become 4 CPUs / the saved 4g, which sbx refuses at create.
    let fit = size(Some(4), None).unwrap();
    assert_eq!(kept(&fit), (4, "8g"));
    let fit = size(None, Some("16G")).unwrap();
    assert_eq!(kept(&fit), (8, "16g"));
    // Both named and not a shape: refused before anything is created, naming the shapes.
    let error = size(Some(4), Some("4g")).unwrap_err();
    assert!(error.contains("2 CPU / 4g, 4 CPU / 8g"), "{error}");
    assert!(size(Some(3), None).is_err());
    assert!(size(None, Some("6g")).is_err());
    // Neither: the saved size fitted to the devcontainer.
    let fit = requested_size(&saved, None, None, &asks(Some(4), Some(8))).unwrap();
    assert_eq!(kept(&fit), (4, "8g"));
    assert!(fit.note.is_some());
    // Named explicitly: kept even under the requirement (the caller chose it), no note.
    let fit = requested_size(&saved, Some(2), Some("4g"), &asks(Some(4), Some(8))).unwrap();
    assert_eq!(kept(&fit), (2, "4g"));
    assert_eq!(fit.note, None);
}
