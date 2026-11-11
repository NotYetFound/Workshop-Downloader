// Remote content gets only status labels. Navigation requests open a trusted
// preview; no filesystem, process, path or authentication API is exposed here.
(() => {
  if (window.top !== window || location.origin !== 'https://steamcommunity.com') return;
  const badgeAttribute = 'data-workshop-desk-badge';
  const selected = new Set();
  let statuses = new Map();
  let scheduled = false;
  let bulk;
  function workshopId(href) {
    let url;
    try { url = new URL(href, location.href); } catch { return null; }
    if (url.origin !== location.origin || !['/sharedfiles/filedetails', '/workshop/filedetails'].includes(url.pathname.replace(/\/+$/, ''))) return null;
    const [id, duplicate] = url.searchParams.getAll('id');
    if (duplicate !== undefined || !id || !/^[0-9]{1,20}$/.test(id)) return null;
    const value = BigInt(id);
    return value > 0n && value <= 18446744073709551615n ? value.toString() : null;
  }
  function request(ids) {
    const url = new URL(location.href);
    url.searchParams.set('workshopdesk_preview', [...new Set(ids)].join(','));
    history.replaceState(history.state, '', url.href);
  }
  function cardInfo(link, id) {
    for (let card = link.parentElement, depth = 0; card && depth < 7; card = card.parentElement, depth++) {
      if (card === document.body) break;
      const links = [...card.querySelectorAll('a[href*="/filedetails"]')];
      if (links.some((entry) => { const found = workshopId(entry.href); return found && found !== id; })) break;
      if (!card.querySelector('img') || !links.some((entry) => entry.textContent.trim() && workshopId(entry.href) === id)) continue;
      const star = card.querySelector('svg.SVGIcon_Star_Filled, svg.SVGIcon_Star_Empty');
      const legacy = card.querySelector('.workshopItemRating');
      const row = star ? (star.closest('.tool-tip-source') ?? star.parentElement)
        : legacy ? (legacy.tagName === 'IMG' ? legacy.parentElement : legacy) : null;
      if (row) {
        row.style.display = 'flex'; row.style.alignItems = 'center';
        row.style.gap = '6px'; row.style.justifyContent = 'space-between';
        return { card, row };
      }
    }
    return null;
  }
  const labels = { downloaded: 'Downloaded', downloadedExternally: 'External', queued: 'Queued', downloading: 'Downloading', missingFiles: 'Missing files', failed: 'Failed' };
  function badge(id) {
    const element = document.createElement('span');
    element.setAttribute(badgeAttribute, id);
    element.style.cssText = 'display:inline-block;flex:0 0 auto;margin-left:6px;padding:1px 5px;border-radius:4px;pointer-events:none;font:10px/16px system-ui,sans-serif;white-space:nowrap';
    return element;
  }
  function updateBadge(element, status) {
    const label = labels[status];
    if (element.textContent !== label) element.textContent = label;
    element.title = status === 'downloadedExternally' ? 'Downloaded externally' : label;
    const failure = ['missingFiles', 'failed'].includes(status);
    const active = ['queued', 'downloading'].includes(status);
    element.style.background = failure ? '#422c2c' : active ? '#253c50' : '#29382f';
    element.style.color = failure ? '#e9b2ad' : active ? '#a4d2f5' : '#add7ba';
  }
  function button(text, title, action) {
    const element = document.createElement('button');
    element.type = 'button'; element.textContent = text; element.title = title;
    element.setAttribute('aria-label', title);
    element.style.cssText = 'border:1px solid #55616d;border-radius:3px;background:#1b2838ee;color:#d6e4f0;padding:3px 6px;cursor:pointer;font:11px system-ui,sans-serif';
    element.addEventListener('click', (event) => { event.preventDefault(); event.stopPropagation(); action(); });
    return element;
  }
  function updateBulk() {
    if (!bulk && selected.size) {
      bulk = button('', 'Preview selected Workshop items', () => request(selected));
      bulk.setAttribute('data-workshop-desk-bulk', '');
      bulk.style.cssText += ';position:fixed;right:16px;bottom:16px;z-index:2147483646;padding:8px 12px;box-shadow:0 3px 12px #0008';
      document.body.append(bulk);
    }
    if (bulk) {
      const text = `Download Selected (${selected.size})`;
      if (bulk.textContent !== text) bulk.textContent = text;
      bulk.hidden = !selected.size;
    }
  }
  function controls(card, id) {
    let control = card.querySelector(':scope > [data-workshop-desk-controls]');
    if (control?.getAttribute('data-workshop-desk-controls') === id) return;
    control?.remove();
    control = document.createElement('div'); control.setAttribute('data-workshop-desk-controls', id);
    control.style.cssText = 'position:absolute;top:5px;left:5px;right:5px;display:flex;justify-content:space-between;align-items:center;z-index:20;pointer-events:none';
    const check = document.createElement('input'); check.type = 'checkbox'; check.checked = selected.has(id);
    check.setAttribute('aria-label', `Select Workshop item ${id}`);
    check.style.cssText = 'pointer-events:auto;width:15px;height:15px;accent-color:#3b83b9;margin:0;cursor:pointer';
    check.addEventListener('click', (event) => event.stopPropagation());
    check.addEventListener('change', () => {
      if (check.checked && selected.size >= 2000) { check.checked = false; return; }
      if (check.checked) selected.add(id); else selected.delete(id);
      updateBulk();
    });
    const download = button('↓', 'Download Workshop item ' + id, () => request([id]));
    download.style.pointerEvents = 'auto';
    control.append(check, download); card.style.position = 'relative'; card.append(control);
  }
  function update() {
    scheduled = false;
    if (!document.body) return;
    const current = workshopId(location.href);
    const hosts = new Map();
    if (current && labels[statuses.get(current)]) {
      const title = document.querySelector('.workshopItemTitle');
      if (title) hosts.set(title, current);
    }
    const browse = /\/(workshop\/browse|app\/\d+\/workshop)/.test(location.pathname);
    if (browse) {
      const cards = new Map();
      for (const link of document.querySelectorAll('a[href*="/filedetails"]')) {
        const id = workshopId(link.href);
        if (!id) continue;
        const info = cardInfo(link, id);
        if (info && !cards.has(info.card)) {
          cards.set(info.card, id); controls(info.card, id);
          if (labels[statuses.get(id)]) hosts.set(info.row, id);
        }
      }
    }
    for (const existing of document.querySelectorAll(`[${badgeAttribute}]`)) {
      const id = existing.getAttribute(badgeAttribute), parent = existing.parentElement;
      if (hosts.get(parent) === id) { updateBadge(existing, statuses.get(id)); hosts.delete(parent); }
      else existing.remove();
    }
    for (const [host, id] of hosts) { const element = badge(id); updateBadge(element, statuses.get(id)); host.append(element); }
    updateBulk();
  }
  function schedule() { if (!scheduled) { scheduled = true; requestAnimationFrame(update); } }
  window.addEventListener('workshop-desk-downloaded', ({ detail }) => {
    if (!detail || typeof detail !== 'object' || Array.isArray(detail)) return;
    statuses = new Map(Object.entries(detail).filter(([id,status]) => /^[0-9]{1,20}$/.test(id) && typeof status === 'string'));
    schedule();
  });
  function start() {
    schedule();
    new MutationObserver(schedule).observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['href', 'data-publishedfileid'] });
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true }); else start();
})();
