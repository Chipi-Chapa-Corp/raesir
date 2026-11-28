use crate::{adapters, ea, epic, model::*, steam};

pub fn scan(
    options: &ScanOptions,
    owned: &OwnedLibrary,
    online: &[EpicOnlineLibrary],
    previous: Option<&Report>,
) -> Report {
    let mut report = steam::scan(options, owned, previous);
    let prefixes = crate::discovery::prefixes(&report);
    let ((epic, epic_warnings), (ea, ea_warnings)) = std::thread::scope(|scope| {
        let ea = scope.spawn(|| ea::scan(&report, previous.map(|r| &r.ea), &prefixes));
        let epic = epic::scan(
            options,
            &report,
            online,
            previous.map(|r| &r.epic),
            &prefixes,
        );
        let ea = ea.join().unwrap_or_else(|_| {
            (
                EaReport::default(),
                vec!["EA library scan stopped unexpectedly.".into()],
            )
        });
        (epic, ea)
    });
    report.epic = epic;
    report.warnings.extend(epic_warnings);
    report.ea = ea;
    report.warnings.extend(ea_warnings);
    let (catalog, warnings) = adapters::scan(&report, previous, prefixes);
    report.catalog = catalog;
    crate::catalog::remove_host_mount_aliases(&mut report.catalog);
    report.linux_systems = crate::linux_support::scan(&report);
    crate::catalog::hydrate_os(&mut report);
    crate::catalog::hydrate_install_actions(&mut report);
    report.scanned_at = crate::util::unix_now();
    report.warnings.extend(warnings);
    report.warnings.sort();
    report.warnings.dedup();
    report
}
