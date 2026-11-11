(() => {
  if (window.top !== window || location.origin !== 'https://steamcommunity.com'
    || !/^\/(sharedfiles|workshop)\/filedetails\/?$/.test(location.pathname)) return;

  function install() {
    const stage = document.getElementById('highlight_player_area');
    const strip = document.getElementById('highlight_strip');
    if (!stage || !strip) return;

    const screenshots = new Map();
    for (const thumb of strip.querySelectorAll('.highlight_strip_item[id^="thumb_"]')) {
      const key = thumb.id.slice('thumb_'.length);
      if (!/^(screenshot|movie)_\d+$/.test(key)) continue;
      const slide = document.getElementById('highlight_' + key);
      const thumbnail = thumb.querySelector('img');
      if (!slide || !stage.contains(slide) || !thumbnail?.src) continue;

      // Reuse the tiny strip asset while Steam loads the original GIF or player.
      // The native image and iframe still render above the poster.
      const poster = new URL(thumbnail.src);
      if (poster.protocol !== 'https:') continue;
      const movie = key.startsWith('movie_');
      const image = movie ? null : slide.querySelector('.screenshot_holder img');
      if (!movie && !image) continue;
      slide.style.backgroundImage = `url(${JSON.stringify(poster.href)})`;
      slide.style.backgroundPosition = 'center';
      slide.style.backgroundRepeat = 'no-repeat';
      slide.style.backgroundSize = 'contain';
      if (image) {
        screenshots.set(thumb, image);
        const clearPoster = () => {
          // Steam seeds every preview with a transparent 1×1 image.
          if (image.naturalWidth > 1 && image.naturalHeight > 1) {
            slide.style.removeProperty('background-image');
          }
        };
        image.addEventListener('load', clearPoster);
        if (image.complete) clearPoster();
      }
    }

    let prioritized;
    function prioritize(event) {
      const image = screenshots.get(event.target.closest('.highlight_strip_item'));
      if (image === prioritized) return;
      if (prioritized) prioritized.removeAttribute('fetchpriority');
      prioritized = image;
      if (image) image.setAttribute('fetchpriority', 'high');
    }
    // Steam loads the full image on focus/click and also preloads its neighbor.
    // Set priority during capture so the selected request gets a head start.
    for (const event of ['pointerdown', 'focus', 'click']) {
      strip.addEventListener(event, prioritize, true);
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', install, { once: true });
  } else {
    install();
  }
})();
