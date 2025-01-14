use std::{collections::BTreeMap, time::Duration};

use average::{concatenate, Estimate, Max, Mean, Min};
use hyper::StatusCode;

use crate::client::{ClientError, RequestResult};

/// Data container for the results of the all requests
/// When a request is successful, the result is pushed to the `success` vector and the memory consumption will not be a problem because the number of successful requests is limited by network overhead.
/// When a request fails, the error message is pushed to the `error` map because the number of error messages may huge.
#[derive(Debug, Default)]
pub struct ResultData {
    success: Vec<RequestResult>,
    error_distribution: BTreeMap<String, usize>,
}

// https://github.com/vks/average/issues/19
// can't define pub struct for now.
concatenate!(MinMaxMeanInner, [Min, min], [Max, max], [Mean, mean]);
pub struct MinMaxMean(MinMaxMeanInner);

impl MinMaxMean {
    pub fn min(&self) -> f64 {
        self.0.min()
    }

    pub fn max(&self) -> f64 {
        self.0.max()
    }

    pub fn mean(&self) -> f64 {
        self.0.mean()
    }
}

impl ResultData {
    #[inline]
    pub fn push(&mut self, result: Result<RequestResult, ClientError>) {
        match result {
            Ok(result) => self.success.push(result),
            Err(err) => {
                let count = self.error_distribution.entry(err.to_string()).or_insert(0);
                *count += 1;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.success.len() + self.error_distribution.values().sum::<usize>()
    }

    // An existence of this method doesn't prevent us to using hdrhistogram.
    // Because this is only called from `monitor` and `monitor` can collect own data.
    pub fn success(&self) -> &[RequestResult] {
        &self.success
    }

    // It's very happy if you can provide all below methods without array (= non liner memory consumption) and fast `push` runtime.

    pub fn success_rate(&self) -> f64 {
        let dead_line = ClientError::Deadline.to_string();
        // We ignore deadline errors which are because of `-z` option, not because of the server
        let denominator = self.success.len()
            + self
                .error_distribution
                .iter()
                .filter_map(|(k, v)| if k == &dead_line { None } else { Some(v) })
                .sum::<usize>();
        let numerator = self.success.len();

        numerator as f64 / denominator as f64
    }

    pub fn latency_stat(&self) -> MinMaxMean {
        let latencies = self
            .success
            .iter()
            .map(|result| result.duration().as_secs_f64())
            .collect::<MinMaxMeanInner>();
        MinMaxMean(latencies)
    }

    pub fn error_distribution(&self) -> &BTreeMap<String, usize> {
        &self.error_distribution
    }

    pub fn end_times_from_start(&self) -> impl Iterator<Item = Duration> + '_ {
        self.success.iter().map(|result| result.end - result.start)
    }

    pub fn status_code_distribution(&self) -> BTreeMap<StatusCode, usize> {
        let mut dist = BTreeMap::new();
        for result in &self.success {
            let count = dist.entry(result.status).or_insert(0);
            *count += 1;
        }
        dist
    }

    pub fn dns_dialup_stat(&self) -> MinMaxMean {
        let dns_dialup = self
            .success
            .iter()
            .filter_map(|r| {
                r.connection_time
                    .map(|ct| (ct.dialup - r.start).as_secs_f64())
            })
            .collect::<MinMaxMeanInner>();
        MinMaxMean(dns_dialup)
    }

    pub fn dns_lookup_stat(&self) -> MinMaxMean {
        let dns_lookup = self
            .success
            .iter()
            .filter_map(|r| {
                r.connection_time
                    .map(|ct| (ct.dns_lookup - r.start).as_secs_f64())
            })
            .collect::<MinMaxMeanInner>();
        MinMaxMean(dns_lookup)
    }

    pub fn total_data(&self) -> usize {
        self.success.iter().map(|r| r.len_bytes).sum()
    }

    pub fn size_per_request(&self) -> Option<u64> {
        self.success
            .iter()
            .map(|r| r.len_bytes as u64)
            .sum::<u64>()
            .checked_div(self.success.len() as u64)
    }

    pub fn duration_all(&self) -> impl Iterator<Item = Duration> + '_ {
        self.success.iter().map(|r| r.duration())
    }

    pub fn duration_successful(&self) -> impl Iterator<Item = Duration> + '_ {
        self.success
            .iter()
            .filter(|r| r.status.is_success())
            .map(|r| r.duration())
    }

    pub fn duration_not_successful(&self) -> impl Iterator<Item = Duration> + '_ {
        self.success
            .iter()
            .filter(|r| !r.status.is_success())
            .map(|r| r.duration())
    }
}

#[cfg(test)]
mod tests {
    use float_cmp::assert_approx_eq;

    use super::*;
    use crate::client::{ClientError, ConnectionTime, RequestResult};
    use std::time::{Duration, Instant};

    fn build_mock_request_result(
        status: StatusCode,
        request_time: u64,
        connection_time_dns_lookup: u64,
        connection_time_dialup: u64,
        size: usize,
    ) -> Result<RequestResult, ClientError> {
        let now = Instant::now();
        Ok(RequestResult {
            start_latency_correction: None,
            start: now,
            connection_time: Some(ConnectionTime {
                dns_lookup: now
                    .checked_add(Duration::from_millis(connection_time_dns_lookup))
                    .unwrap(),
                dialup: now
                    .checked_add(Duration::from_millis(connection_time_dialup))
                    .unwrap(),
            }),
            end: now
                .checked_add(Duration::from_millis(request_time))
                .unwrap(),
            status,
            len_bytes: size,
        })
    }

    fn build_mock_request_results() -> ResultData {
        let mut results = ResultData::default();

        results.push(build_mock_request_result(
            StatusCode::OK,
            1000,
            200,
            50,
            100,
        ));
        results.push(build_mock_request_result(
            StatusCode::BAD_REQUEST,
            100000,
            250,
            100,
            200,
        ));
        results.push(build_mock_request_result(
            StatusCode::INTERNAL_SERVER_ERROR,
            1000000,
            300,
            150,
            300,
        ));
        results
    }

    #[test]
    fn test_calculate_success_rate() {
        let res = build_mock_request_results();
        assert_approx_eq!(f64, res.success_rate(), 1.0);
    }

    #[test]
    fn test_calculate_slowest_request() {
        let res = build_mock_request_results();
        assert_approx_eq!(f64, res.latency_stat().max(), 1000.0);
    }

    #[test]
    fn test_calculate_average_request() {
        let res = build_mock_request_results();
        assert_approx_eq!(f64, res.latency_stat().mean(), 367.0);
    }

    #[test]
    fn test_calculate_total_data() {
        let res = build_mock_request_results();
        assert_eq!(res.total_data(), 600);
    }

    #[test]
    fn test_calculate_size_per_request() {
        let res = build_mock_request_results();
        assert_eq!(res.size_per_request(), Some(200));
    }

    #[test]
    fn test_calculate_connection_times_dns_dialup_average() {
        let res = build_mock_request_results();
        assert_approx_eq!(f64, res.dns_dialup_stat().mean(), 0.1);
    }

    #[test]
    fn test_calculate_connection_times_dns_dialup_fastest() {
        let res = build_mock_request_results();
        assert_approx_eq!(f64, res.dns_dialup_stat().min(), 0.05);
    }

    #[test]
    fn test_calculate_connection_times_dns_dialup_slowest() {
        let res = build_mock_request_results();
        assert_approx_eq!(f64, res.dns_dialup_stat().max(), 0.15);
    }

    #[test]
    fn test_calculate_connection_times_dns_lookup_average() {
        let res = build_mock_request_results();
        assert_approx_eq!(f64, res.dns_lookup_stat().mean(), 0.25);
    }

    #[test]
    fn test_calculate_connection_times_dns_lookup_fastest() {
        let res = build_mock_request_results();
        assert_approx_eq!(f64, res.dns_lookup_stat().min(), 0.2);
    }

    #[test]
    fn test_calculate_connection_times_dns_lookup_slowest() {
        let res = build_mock_request_results();
        assert_approx_eq!(f64, res.dns_lookup_stat().max(), 0.3);
    }

    #[test]
    fn test_get_durations_all() {
        let res = build_mock_request_results();
        let durations: Vec<_> = res.duration_all().map(|d| d.as_secs_f64()).collect();
        assert_approx_eq!(f64, durations[0], 1.0);
        assert_approx_eq!(f64, durations[1], 100.0);
        assert_approx_eq!(f64, durations[2], 1000.0);
    }

    #[test]
    fn test_get_durations_successful() {
        let res = build_mock_request_results();
        let durations: Vec<_> = res.duration_successful().map(|d| d.as_secs_f64()).collect();
        // Round the calculations to 4 decimal places to remove imprecision
        assert_approx_eq!(f64, durations[0], 1.0);
        assert_eq!(durations.get(1), None);
    }

    #[test]
    fn test_get_durations_not_successful() {
        let res = build_mock_request_results();
        let durations: Vec<_> = res
            .duration_not_successful()
            .map(|d| d.as_secs_f64())
            .collect();
        // Round the calculations to 4 decimal places to remove imprecision
        assert_approx_eq!(f64, durations[0], 100.0);
        assert_approx_eq!(f64, durations[1], 1000.0);
        assert_eq!(durations.get(2), None);
    }
}
