// Tiny fuzzy matcher — subsequence with bonuses for matching at the
// start and for consecutive runs. Returns a positive score (higher is
// better) or null when the query is not a subsequence of the target.
//
// Same matcher is used across every results-pane context (recents,
// local dir listing, remote repo listing) so the ranking feels
// consistent everywhere.

const START_BONUS = 12;
const CONSECUTIVE_BASE = 5;
const PLAIN_HIT = 1;

export function fuzzyScore(query: string, target: string): number | null {
  if (!query) return 1;
  const q = query.toLowerCase();
  const t = target.toLowerCase();
  let qi = 0;
  let score = 0;
  let prevMatched = false;
  let consecutive = 0;

  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) {
      if (ti === 0) score += START_BONUS;
      if (prevMatched) {
        consecutive += 1;
        score += CONSECUTIVE_BASE + consecutive;
      } else {
        score += PLAIN_HIT;
      }
      prevMatched = true;
      qi += 1;
    } else {
      prevMatched = false;
      consecutive = 0;
    }
  }

  return qi === q.length ? score : null;
}

/**
 * Filter + rank a list of items. The `pick` function returns the
 * string(s) to match against — when given multiple, the best match
 * across them wins. Items with no match are dropped; ties preserve
 * original order (stable sort).
 */
export function fuzzyFilter<T>(
  items: readonly T[],
  query: string,
  pick: (item: T) => string | string[],
): T[] {
  if (!query) return [...items];
  const scored: Array<{ item: T; score: number; idx: number }> = [];
  items.forEach((item, idx) => {
    const candidates = pick(item);
    const arr = Array.isArray(candidates) ? candidates : [candidates];
    let best: number | null = null;
    for (const s of arr) {
      const sc = fuzzyScore(query, s);
      if (sc != null && (best == null || sc > best)) best = sc;
    }
    if (best != null) scored.push({ item, score: best, idx });
  });
  scored.sort((a, b) => b.score - a.score || a.idx - b.idx);
  return scored.map((s) => s.item);
}
