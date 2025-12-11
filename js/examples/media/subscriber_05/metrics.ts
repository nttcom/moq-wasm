
const API_URL = "https://192.168.40.228:8080/api/metrics";

const metricMeta: Record<string, { lastSent: number; intervalMs: number; latestValue: number }> = {};

async function fetchWithTimeout(url: string, options: any, timeoutMs = 3000) {
  const controller = new AbortController();
  const id = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res = await fetch(url, { ...options, signal: controller.signal });
    clearTimeout(id);
    return res;
  } catch (err) {
    clearTimeout(id);
    throw err;
  }
}

/**
 * postMetrics (イベント駆動型)
 * 値を受け取るたびに「前回送信から intervalMs 経過していれば」送信する。
 *
 * @param client_type クライアントのタイプタイプ{ 'pub', 'sub' }
 * @param parm_key メトリクス名
 * @param value 値
 * @param intervalMs このメトリクスを送信する最小間隔(ms)
 */
export async function postMetrics(client_type: string | '', param_key: string | null, value: number | null, intervalMs = 1000) {
  if (!param_key || value == null) return;

  const key = `${client_type}_${param_key}`
  const now = Date.now();
  const meta = metricMeta[key] ?? { lastSent: 0, intervalMs, latestValue: value };

  meta.latestValue = value;
  meta.intervalMs = intervalMs;

  // 🔹 Interval 経過チェック
  if (now - meta.lastSent >= meta.intervalMs) {
    meta.lastSent = now;
    await sendMetricOnce(key, value, intervalMs);
  } else {
    const remain = meta.intervalMs - (now - meta.lastSent);
    //console.debug(`⏸️ skip ${key} (${remain.toFixed(0)}ms remaining)`);
  }

  metricMeta[key] = meta;
}

async function sendMetricOnce(key: string, value: number, intervalMs: number) {
  const data: Record<string, any> = { [key]: value };
/*
  try {
    const res = await fetchWithTimeout(`${API_URL}?interval=${intervalMs}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(data),
    }, 3000);

    if (!res.ok) throw new Error(`HTTP ${res.status}`);

    //console.log(`✅ sent ${key}: ${value} (interval ${intervalMs}ms)`);
  } catch (err: any) {
    console.error(`❌ failed to send ${key}:`, err.message);
  }
*/
}




/*
const API_URL = "http://192.168.40.228:8080/api/metrics";

// タイムアウト付き fetch 関数
async function fetchWithTimeout(url, options = {}, timeoutMs = 3000) {
    const controller = new AbortController();
    const id = setTimeout(() => controller.abort(), timeoutMs);

    try {
        const res = await fetch(url, { ...options, signal: controller.signal });
        clearTimeout(id);
        return res;
    } catch (err) {
        clearTimeout(id);
        if (err.name === "AbortError") {
            throw new Error(`Request timed out after ${timeoutMs}ms`);
        }
        throw err;
    }
}

// メトリクス送信関数
export async function postMetrics( key = null, value = null) {
    if ( key === null || value === null ) return;
    const data: Record<string, any> = {};
    data[key] = value;

    try {
        const res = await fetchWithTimeout(API_URL, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(data),
        }, 3000);

        if (!res.ok) {
            throw new Error(`HTTP ${res.status}`);
        }

        const json = await res.json();
        //console.log("✅ sent:", json);
    } catch (err) {
        console.error("❌ failed:", err.message);
    }
}
*/
