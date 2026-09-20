/** Exact UI-side copy of the canonical V4 internal seat-id rule (§4.1). */
export const SEAT_NAME_RE = /^[a-z0-9][a-z0-9_-]{0,30}$/;

export function isValidSeatName(name: string): boolean {
  return SEAT_NAME_RE.test(name);
}
