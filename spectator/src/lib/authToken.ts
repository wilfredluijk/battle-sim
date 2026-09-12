export function storedToken(): string | null {
  try { return localStorage.getItem('naval.adminToken'); } catch { return null; }
}
export function storedAuthHeaders(): Record<string, string> {
  const token = storedToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}
