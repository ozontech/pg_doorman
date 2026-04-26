// Populate the sidebar
//
// This is a script, and not included directly in the page, to control the total size of the book.
// The TOC contains an entry for each page, so if each page includes a copy of the TOC,
// the total size of the page becomes O(n**2).
class MDBookSidebarScrollbox extends HTMLElement {
    constructor() {
        super();
    }
    connectedCallback() {
        this.innerHTML = '<ol class="chapter"><li class="chapter-item expanded affix "><a href="index.html">Home</a></li><li class="chapter-item expanded affix "><a href="comparison.html">Comparison</a></li><li class="chapter-item expanded affix "><li class="spacer"></li><li class="chapter-item expanded affix "><li class="part-title">Getting Started</li><li class="chapter-item expanded "><a href="tutorials/overview.html"><strong aria-hidden="true">1.</strong> Overview</a></li><li class="chapter-item expanded "><a href="tutorials/installation.html"><strong aria-hidden="true">2.</strong> Installation</a></li><li class="chapter-item expanded "><a href="tutorials/basic-usage.html"><strong aria-hidden="true">3.</strong> Basic Usage</a></li><li class="chapter-item expanded affix "><li class="part-title">Authentication</li><li class="chapter-item expanded "><a href="authentication/overview.html"><strong aria-hidden="true">4.</strong> Overview</a></li><li class="chapter-item expanded "><a href="authentication/passthrough.html"><strong aria-hidden="true">5.</strong> Passthrough (default)</a></li><li class="chapter-item expanded "><a href="authentication/auth-query.html"><strong aria-hidden="true">6.</strong> auth_query</a></li><li class="chapter-item expanded "><a href="authentication/pam.html"><strong aria-hidden="true">7.</strong> PAM</a></li><li class="chapter-item expanded "><a href="authentication/jwt.html"><strong aria-hidden="true">8.</strong> JWT</a></li><li class="chapter-item expanded "><a href="authentication/talos.html"><strong aria-hidden="true">9.</strong> Talos</a></li><li class="chapter-item expanded "><a href="authentication/hba.html"><strong aria-hidden="true">10.</strong> pg_hba.conf</a></li><li class="chapter-item expanded affix "><li class="part-title">TLS</li><li class="chapter-item expanded "><a href="guides/tls.html"><strong aria-hidden="true">11.</strong> Client and Server TLS</a></li><li class="chapter-item expanded affix "><li class="part-title">Pooling</li><li class="chapter-item expanded "><a href="concepts/pool-modes.html"><strong aria-hidden="true">12.</strong> Pool Modes</a></li><li class="chapter-item expanded "><a href="concepts/pool-coordinator.html"><strong aria-hidden="true">13.</strong> Pool Coordinator</a></li><li class="chapter-item expanded "><a href="tutorials/pool-pressure.html"><strong aria-hidden="true">14.</strong> Pool Pressure (advanced)</a></li><li class="chapter-item expanded affix "><li class="part-title">High Availability</li><li class="chapter-item expanded "><a href="tutorials/patroni-assisted-fallback.html"><strong aria-hidden="true">15.</strong> Patroni-assisted Fallback</a></li><li class="chapter-item expanded "><a href="tutorials/patroni-proxy.html"><strong aria-hidden="true">16.</strong> patroni_proxy</a></li><li class="chapter-item expanded affix "><li class="part-title">Operations</li><li class="chapter-item expanded "><a href="tutorials/binary-upgrade.html"><strong aria-hidden="true">17.</strong> Binary Upgrade</a></li><li class="chapter-item expanded "><a href="operations/signals.html"><strong aria-hidden="true">18.</strong> Signals and Reload</a></li><li class="chapter-item expanded "><a href="tutorials/troubleshooting.html"><strong aria-hidden="true">19.</strong> Troubleshooting</a></li><li class="chapter-item expanded affix "><li class="part-title">Observability</li><li class="chapter-item expanded "><a href="observability/admin-commands.html"><strong aria-hidden="true">20.</strong> Admin Commands</a></li><li class="chapter-item expanded "><a href="observability/json-logging.html"><strong aria-hidden="true">21.</strong> JSON Structured Logging</a></li><li class="chapter-item expanded "><a href="observability/percentiles.html"><strong aria-hidden="true">22.</strong> Latency Percentiles</a></li><li class="chapter-item expanded affix "><li class="part-title">Reference</li><li class="chapter-item expanded "><a href="reference/general.html"><strong aria-hidden="true">23.</strong> General Settings</a></li><li class="chapter-item expanded "><a href="reference/pool.html"><strong aria-hidden="true">24.</strong> Pool Settings</a></li><li class="chapter-item expanded "><a href="reference/prometheus.html"><strong aria-hidden="true">25.</strong> Prometheus Settings</a></li><li class="chapter-item expanded affix "><li class="spacer"></li><li class="chapter-item expanded "><a href="benchmarks.html"><strong aria-hidden="true">26.</strong> Benchmarks</a></li><li class="chapter-item expanded "><a href="changelog.html"><strong aria-hidden="true">27.</strong> Changelog</a></li><li class="chapter-item expanded "><a href="tutorials/contributing.html"><strong aria-hidden="true">28.</strong> Contributing</a></li></ol>';
        // Set the current, active page, and reveal it if it's hidden
        let current_page = document.location.href.toString().split("#")[0];
        if (current_page.endsWith("/")) {
            current_page += "index.html";
        }
        var links = Array.prototype.slice.call(this.querySelectorAll("a"));
        var l = links.length;
        for (var i = 0; i < l; ++i) {
            var link = links[i];
            var href = link.getAttribute("href");
            if (href && !href.startsWith("#") && !/^(?:[a-z+]+:)?\/\//.test(href)) {
                link.href = path_to_root + href;
            }
            // The "index" page is supposed to alias the first chapter in the book.
            if (link.href === current_page || (i === 0 && path_to_root === "" && current_page.endsWith("/index.html"))) {
                link.classList.add("active");
                var parent = link.parentElement;
                if (parent && parent.classList.contains("chapter-item")) {
                    parent.classList.add("expanded");
                }
                while (parent) {
                    if (parent.tagName === "LI" && parent.previousElementSibling) {
                        if (parent.previousElementSibling.classList.contains("chapter-item")) {
                            parent.previousElementSibling.classList.add("expanded");
                        }
                    }
                    parent = parent.parentElement;
                }
            }
        }
        // Track and set sidebar scroll position
        this.addEventListener('click', function(e) {
            if (e.target.tagName === 'A') {
                sessionStorage.setItem('sidebar-scroll', this.scrollTop);
            }
        }, { passive: true });
        var sidebarScrollTop = sessionStorage.getItem('sidebar-scroll');
        sessionStorage.removeItem('sidebar-scroll');
        if (sidebarScrollTop) {
            // preserve sidebar scroll position when navigating via links within sidebar
            this.scrollTop = sidebarScrollTop;
        } else {
            // scroll sidebar to current active section when navigating via "next/previous chapter" buttons
            var activeSection = document.querySelector('#sidebar .active');
            if (activeSection) {
                activeSection.scrollIntoView({ block: 'center' });
            }
        }
        // Toggle buttons
        var sidebarAnchorToggles = document.querySelectorAll('#sidebar a.toggle');
        function toggleSection(ev) {
            ev.currentTarget.parentElement.classList.toggle('expanded');
        }
        Array.from(sidebarAnchorToggles).forEach(function (el) {
            el.addEventListener('click', toggleSection);
        });
    }
}
window.customElements.define("mdbook-sidebar-scrollbox", MDBookSidebarScrollbox);
