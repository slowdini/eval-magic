//! Statistical planning helpers for the pre-dispatch run summary.

/// Format the smallest two-sided Fisher exact p-value attainable from two
/// equally sized binary samples: perfect separation, `runs/runs` versus `0/runs`.
pub(super) fn format_minimum_attainable_fisher_p_value(runs: u32) -> String {
    debug_assert!(runs > 0);
    if runs == 1 {
        return "1.0".to_string();
    }

    // At perfect separation p = 2 / C(2n, n). Summing the binomial's factors
    // in log space keeps useful scientific notation long after f64 would
    // underflow the probability itself to zero.
    let log10_binomial = (1..=runs)
        .map(|k| ((runs as f64 + k as f64) / k as f64).log10())
        .sum::<f64>();
    let log10_p = 2.0_f64.log10() - log10_binomial;

    if log10_p >= -3.0 {
        let p = 10.0_f64.powf(log10_p);
        // Absorb accumulated error at exact powers of ten, such as n=3's 0.10.
        let magnitude = (log10_p + 1e-12).floor() as i32;
        let decimal_places = (1 - magnitude).max(0) as usize;
        return format!("{p:.decimal_places$}");
    }

    let mut exponent = log10_p.floor() as i32;
    let mut mantissa = 10.0_f64.powf(log10_p - f64::from(exponent));
    mantissa = (mantissa * 10.0).round() / 10.0;
    if mantissa >= 10.0 {
        mantissa = 1.0;
        exponent += 1;
    }
    format!("{mantissa:.1}e{exponent}")
}

#[cfg(test)]
mod tests {
    use super::format_minimum_attainable_fisher_p_value;

    #[test]
    fn attainable_fisher_floor_formats_known_run_counts() {
        assert_eq!(format_minimum_attainable_fisher_p_value(1), "1.0");
        assert_eq!(format_minimum_attainable_fisher_p_value(3), "0.10");
        assert_eq!(format_minimum_attainable_fisher_p_value(4), "0.029");
        assert_eq!(format_minimum_attainable_fisher_p_value(10), "1.1e-5");
    }

    #[test]
    fn attainable_fisher_floor_does_not_underflow_to_zero() {
        let formatted = format_minimum_attainable_fisher_p_value(1_000);
        assert!(formatted.contains("e-"), "{formatted}");
        assert_ne!(formatted, "0");
    }
}
