// EN ↔ RU language switcher injected into mdBook header.
// English book lives at the site root, Russian at /<base>/ru/.
(function () {
    var path = window.location.pathname;

    // Detect site base path (e.g. /pg_doorman/ on GitHub Pages, / locally).
    var base = '/';
    var marker = '/pg_doorman/';
    if (path.indexOf(marker) === 0) {
        base = marker;
    }

    var rest = path.substring(base.length);
    var isRu = rest.indexOf('ru/') === 0 || rest === 'ru' || rest === 'ru/';

    var targetPath, label, title;
    if (isRu) {
        targetPath = base + rest.replace(/^ru\/?/, '');
        if (targetPath === base) targetPath = base;
        label = 'EN';
        title = 'Switch to English';
    } else {
        targetPath = base + 'ru/' + rest;
        label = 'RU';
        title = 'Переключиться на русский';
    }

    function injectLink() {
        var rightButtons = document.querySelector('.right-buttons');
        if (!rightButtons) {
            // Header not ready yet.
            return false;
        }
        if (rightButtons.querySelector('.lang-switcher')) {
            return true;
        }
        var link = document.createElement('a');
        link.href = targetPath;
        link.textContent = label;
        link.title = title;
        link.className = 'lang-switcher icon-button';
        link.style.cssText =
            'font-weight: bold; text-decoration: none; padding: 0 8px; ' +
            'display: inline-flex; align-items: center; min-width: 28px; ' +
            'justify-content: center;';
        rightButtons.appendChild(link);
        return true;
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', injectLink);
    } else {
        injectLink();
    }
})();
