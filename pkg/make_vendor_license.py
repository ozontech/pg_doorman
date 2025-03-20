import sys
import json

if len(sys.argv) != 3:
    print("usage: %s vendor.json vendor-license.txt")
    sys.exit(1)

with open(sys.argv[1]) as r:
    with open(sys.argv[2], "w") as w:
        for pkg in json.load(r):
            if (pkg["license"] == ""):
                os.exit(2)
            w.write("Files: vendor/%s/*\nLicense: %s\n\n" % (pkg["name"], pkg["license"]))
