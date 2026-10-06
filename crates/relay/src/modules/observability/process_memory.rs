const PROC_STATUS: &str = "/proc/self/status";

pub(crate) async fn resident_set_bytes() -> Option<u64> {
    let status = tokio::fs::read_to_string(PROC_STATUS).await.ok()?;
    parse_vm_rss_bytes(&status)
}

fn parse_vm_rss_bytes(status: &str) -> Option<u64> {
    let kilobytes = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(kilobytes * 1024)
}

#[cfg(test)]
mod tests {
    use super::parse_vm_rss_bytes;

    #[test]
    fn vm_rss_is_read_in_kibibytes() {
        // Arrange
        let status = "Name:\trelay\nVmPeak:\t  900 kB\nVmRSS:\t  12345 kB\nThreads:\t8\n";

        // Act / Assert
        assert_eq!(parse_vm_rss_bytes(status), Some(12345 * 1024));
    }

    #[test]
    fn status_without_vm_rss_has_no_value() {
        // Act / Assert
        assert_eq!(parse_vm_rss_bytes("Name:\trelay\n"), None);
    }
}
