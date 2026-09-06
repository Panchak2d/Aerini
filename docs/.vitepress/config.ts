import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'Aerini Docs',
  description: 'Documentation for Aerini, a local-first visual workflow automation engine.',
  lang: 'en-US',

  markdown: {
    // Aerini's own {{...}} expression syntax appears throughout docs/ as
    // inline code. Vue's compiler treats {{ }} as interpolation even inside
    // <code>, so without this every literal {{...}} example breaks the build.
    config(md) {
      const defaultCodeInline = md.renderer.rules.code_inline!
      md.renderer.rules.code_inline = (tokens, idx, options, env, self) => {
        tokens[idx].attrSet('v-pre', '')
        return defaultCodeInline(tokens, idx, options, env, self)
      }
    }
  },

  themeConfig: {
    nav: [
      { text: 'Getting Started', link: '/getting-started/getting-started', activeMatch: '/getting-started/' },
      { text: 'Guide', link: '/guide/nodes', activeMatch: '/guide/' },
      { text: 'Reference', link: '/reference/workflow-file-format', activeMatch: '/reference/' },
      { text: 'Operations', link: '/operations/server-deploy', activeMatch: '/operations/' },
      { text: 'Development', link: '/development/architecture', activeMatch: '/development/' },
      { text: 'GitHub', link: 'https://github.com/Panchak2d/Aerini' }
    ],

    sidebar: [
      {
        text: 'Getting Started',
        items: [
          { text: 'Installation', link: '/getting-started/installation' },
          { text: 'Getting Started', link: '/getting-started/getting-started' },
          { text: 'Concepts', link: '/getting-started/concepts' }
        ]
      },
      {
        text: 'Guide',
        items: [
          { text: 'Nodes Reference', link: '/guide/nodes' },
          { text: 'Expressions', link: '/guide/expressions' },
          { text: 'Credentials', link: '/guide/credentials' },
          { text: 'Background Runs', link: '/guide/background-runs' },
          { text: 'Local Models', link: '/guide/local-models' },
          { text: 'Widget Embedding', link: '/guide/widget-embedding' },
          { text: 'Security', link: '/guide/security' },
          { text: 'Examples', link: '/guide/examples' }
        ]
      },
      {
        text: 'Reference',
        items: [
          { text: 'Workflow File Format', link: '/reference/workflow-file-format' },
          { text: 'Schema Migrations', link: '/reference/schema-migrations' }
        ]
      },
      {
        text: 'Operations',
        items: [
          { text: 'Server Deployment', link: '/operations/server-deploy' },
          { text: 'Server CLI Reference', link: '/operations/server-cli-reference' },
          { text: 'Server API Reference', link: '/operations/server-api-reference' },
          { text: 'Updating', link: '/operations/updating' }
        ]
      },
      {
        text: 'Development',
        items: [
          { text: 'Architecture', link: '/development/architecture' },
          { text: 'Adding a Built-in Node', link: '/development/node-authoring' },
          { text: 'Writing a Plugin Node', link: '/development/plugin-authoring' },
          { text: 'Embedding aerini-engine', link: '/development/embedding' },
          { text: 'Testing', link: '/development/testing' },
          { text: 'Desktop IPC Reference', link: '/development/desktop-ipc-reference' }
        ]
      },
      {
        text: 'More',
        items: [
          { text: 'FAQ', link: '/faq' },
          { text: 'Glossary', link: '/glossary' },
          { text: 'Troubleshooting', link: '/troubleshooting' }
        ]
      }
    ],

    search: {
      provider: 'local'
    }
  }
})
