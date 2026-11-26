#!/usr/bin/env python3
"""Check local image selection without Docker, Nix, or registry access."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def run(args, cwd, env):
    return subprocess.run(
        args, cwd=cwd, env=env, check=True, text=True, capture_output=True
    ).stdout


def main():
    source = Path(__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory() as directory:
        scratch = Path(directory)
        root = scratch / "repo"
        for name in (
            "Makefile", "tests/Makefile", "src/bin/patroni_proxy/Makefile",
            "tests/nix/run-tests.sh", "tests/nix/flake.nix", "tests/nix/flake.lock",
        ):
            destination = root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source / name, destination)
        tag = "flake-" + hashlib.sha256(
            (root / "tests/nix/flake.nix").read_bytes()
            + (root / "tests/nix/flake.lock").read_bytes()
        ).hexdigest()[:16]
        docker = scratch / "docker"
        docker.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, sys\n"
            "with open(os.environ['DOCKER_LOG'], 'a') as log:\n"
            "    log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
            "sys.exit(1 if sys.argv[1:3] == ['image', 'inspect'] else 0)\n"
        )
        docker.chmod(0o755)
        env = dict(os.environ, PATH=f"{scratch}:{os.environ['PATH']}")
        for key in ("REGISTRY", "REPO", "IMAGE_TAG", "MAKEFLAGS", "MFLAGS"):
            env.pop(key, None)
        log = scratch / "docker.log"
        env["DOCKER_LOG"] = str(log)
        run(["git", "init", "-q"], root, env)
        cases = 0
        for origin in (
            "https://github.com/vadv/pg_doorman.git",
            "git@github.com:vadv/pg_doorman.git",
            "ssh://git@github.com/vadv/pg_doorman.git",
            "https://github.com/vadv/pg_doorman",
        ):
            run(["git", "config", "remote.origin.url", origin], root, env)
            for overrides in (
                {}, {"REGISTRY": "registry.example:5000"},
                {"REPO": "another/fork"}, {"IMAGE_TAG": "custom"},
                {"REGISTRY": "registry.example", "REPO": "another/fork", "IMAGE_TAG": "custom"},
            ):
                selected = dict(REGISTRY="ghcr.io", REPO="vadv/pg_doorman", IMAGE_TAG=tag)
                selected.update(overrides)
                image = "{REGISTRY}/{REPO}/test-runner:{IMAGE_TAG}".format(**selected)
                for cwd in (root, root / "tests"):
                    for cli in (False, True):
                        current_env = env if cli else dict(env, **overrides)
                        make = ["make", "--no-print-directory"]
                        if cli:
                            make += [f"{key}={value}" for key, value in overrides.items()]
                        build = run(make + ["-n", "local-build"], cwd, current_env)
                        assert f"docker tag pg_doorman-test-env:latest {image}\n" in build, build
                        log.write_text("")
                        run(make + ["pull", "test-bdd", "TAGS=@ldap"], cwd, current_env)
                        calls = [json.loads(line) for line in log.read_text().splitlines()]
                        assert calls[:3] == [["pull", image], ["image", "inspect", image], ["pull", image]], calls
                        assert calls[3][0] == "run" and calls[3][-4:] == [
                            image, "bash", "-c", "cargo test --test bdd -- --tags @ldap"
                        ], calls
                        cases += 1
                log.write_text("")
                run([str(root / "tests/nix/run-tests.sh"), "pull"], scratch, dict(env, **overrides))
                assert json.loads(log.read_text()) == ["pull", image]
                cases += 1
        print(f"PASS: {cases} image-reference cases (origins, overrides, build/pull/BDD, root/tests/direct)")


if __name__ == "__main__":
    main()
