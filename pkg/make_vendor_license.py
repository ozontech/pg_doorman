import sys
import json

if len(sys.argv) != 3:
    print("usage: %s vendor.json vendor-license.txt" % (sys.argv[0]))
    sys.exit(1)

white_list = ["(MIT OR Apache-2.0) AND Unicode-3.0", "0BSD OR Apache-2.0 OR MIT", "Apache-2.0",
    "Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT", "Apache-2.0 OR BSD-2-Clause OR MIT",
    "Apache-2.0 OR BSL-1.0", "Apache-2.0 OR LGPL-2.1-or-later OR MIT", "Apache-2.0 OR MIT",
    "Apache-2.0 OR MIT OR Zlib", "BSD-3-Clause", "MIT", "MIT OR Unlicense", "Zlib", "(Apache-2.0 OR MIT) AND Unicode-3.0"]

with open(sys.argv[1]) as r:
    with open(sys.argv[2], "w") as w:
        for pkg in json.load(r):
            if pkg["license"] not in white_list:
                print("unknown license: %s for package %s" % (pkg["license"], pkg["name"]))
                sys.exit(2)
            w.write("Files: vendor/%s/*\nLicense: %s\n\n" % (pkg["name"], pkg["license"]))
