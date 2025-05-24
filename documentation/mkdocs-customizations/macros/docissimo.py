"""
## Loading attributes

This feature loads a toml file, and import the content into the environment.

You can access values using `{{ attributes.versions.camel}}` or `{{ attributes['project-version'] }}`.

The location can be configured in the `mkdocs.yml` file with:

```
extra:
  attributes_path: docs/my-attributes.toml
```


"""
import toml
import os

def loadAttributes(env):
    path = env.variables['attributes_path']

    with open(path) as attrs_file:
        config = toml.load(attrs_file)
        env.variables['version'] = config['package']['version']

def define_env(env):

    loadAttributes(env)