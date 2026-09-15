// Notifications may contain watch, Shorts, short links or Studio copyright URLs.
// Only extract IDs; never request a URL pasted by the user.
export function notificationVideoIds(text: string): string[] {
  const ids = new Set<string>();
  const valid = (id?: string | null) => { if (id && /^[A-Za-z0-9_-]{11}$/.test(id)) ids.add(id); };
  const rest = text.replace(/https?:\/\/[^\s<>"'，。；）)]+/g, (link) => {
    try {
      const url = new URL(link);
      const parts = url.pathname.split("/").filter(Boolean);
      if (url.hostname === "youtu.be") valid(parts[0]);
      else if (["youtube.com", "www.youtube.com", "m.youtube.com", "studio.youtube.com"].includes(url.hostname)) {
        if (url.pathname === "/watch") valid(url.searchParams.get("v"));
        else if (["shorts", "live", "embed", "video"].includes(parts[0])) valid(parts[1]);
      }
    } catch { /* Other notification text is not a video link. */ }
    return " ";
  });
  // Plain IDs are accepted one per line, or separated by commas/semicolons.
  for (const token of rest.split(/[\n,，;；]+/)) valid(token.trim());
  return [...ids];
}
