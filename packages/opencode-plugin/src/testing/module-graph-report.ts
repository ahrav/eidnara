import { bundleModuleGraph, type ModuleGraph } from "./module-graph";

// Running the bundler in a child process prevents test-runner module mocks from leaking into its module graph.
const graphs: Record<string, Omit<ModuleGraph, "text">> = {};
for (const entry of process.argv.slice(2)) {
    const { inputs, externals, imports } = await bundleModuleGraph(entry);
    graphs[entry] = { inputs, externals, imports };
}
process.stdout.write(JSON.stringify(graphs));
