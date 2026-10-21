const buckets = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10];

// Callers use fixed route and method categories. No identity, query, or path data
// enters labels, so user input cannot create unbounded metric cardinality.
export function createMetrics() {
  const started = process.hrtime.bigint();
  const requests = new Map();
  const durations = new Map();
  let active = 0;

  return {
    begin() {
      active += 1;
      const requestStart = process.hrtime.bigint();
      let complete = false;
      return (method, route, status) => {
        if (complete) return;
        complete = true;
        active -= 1;
        const elapsed = Number(process.hrtime.bigint() - requestStart) / 1e9;
        const labels = `method="${method}",route="${route}"`;
        const counterKey = `${labels},status="${status}"`;
        requests.set(counterKey, (requests.get(counterKey) ?? 0) + 1);
        const series = durations.get(labels) ?? { count: 0, sum: 0, buckets: buckets.map(() => 0) };
        series.count += 1;
        series.sum += elapsed;
        buckets.forEach((bound, index) => {
          if (elapsed <= bound) series.buckets[index] += 1;
        });
        durations.set(labels, series);
      };
    },

    render() {
      const lines = [
        '# HELP obleth_chat_preview_info Information about this preview service.',
        '# TYPE obleth_chat_preview_info gauge',
        'obleth_chat_preview_info{mode="preview"} 1',
        '# HELP obleth_chat_uptime_seconds Seconds since this service instance was created.',
        '# TYPE obleth_chat_uptime_seconds gauge',
        `obleth_chat_uptime_seconds ${Number(process.hrtime.bigint() - started) / 1e9}`,
        '# HELP obleth_chat_http_active_requests Requests currently handled by this instance, including the current scrape.',
        '# TYPE obleth_chat_http_active_requests gauge',
        `obleth_chat_http_active_requests ${active}`,
        '# HELP obleth_chat_http_requests_total Completed HTTP requests, with aborted requests recorded as status 499.',
        '# TYPE obleth_chat_http_requests_total counter',
      ];
      for (const [labels, count] of [...requests].sort(([a], [b]) => a.localeCompare(b))) {
        lines.push(`obleth_chat_http_requests_total{${labels}} ${count}`);
      }
      lines.push(
        '# HELP obleth_chat_http_request_duration_seconds Time until the HTTP response finishes or closes.',
        '# TYPE obleth_chat_http_request_duration_seconds histogram',
      );
      for (const [labels, series] of [...durations].sort(([a], [b]) => a.localeCompare(b))) {
        buckets.forEach((bound, index) => {
          lines.push(`obleth_chat_http_request_duration_seconds_bucket{${labels},le="${bound}"} ${series.buckets[index]}`);
        });
        lines.push(
          `obleth_chat_http_request_duration_seconds_bucket{${labels},le="+Inf"} ${series.count}`,
          `obleth_chat_http_request_duration_seconds_sum{${labels}} ${series.sum}`,
          `obleth_chat_http_request_duration_seconds_count{${labels}} ${series.count}`,
        );
      }
      return `${lines.join('\n')}\n`;
    },
  };
}
