// Type-checked by tsconfig.fixtures.json, never run: Solo JSX must satisfy
// Solid's control flow and render() without casts.
import { createSignal, For, Show } from "solid-js"
import { render, Text, View, VirtualList } from "@solo/solid"

function App() {
  const [items] = createSignal(["a", "b"])
  const [selected] = createSignal<string | null>("a")
  return <View style={{ display: "flex", flexDirection: "column" }}>
    <Show when={items().length > 0} fallback={<Text>Empty</Text>}>
      <Text>Items</Text>
    </Show>
    <Show when={selected()}>{(name) => <Text>{name()}</Text>}</Show>
    <For each={items()}>{(item, index) => <Text>{index()}: {item}</Text>}</For>
    <input onChange={(event) => event.value?.toUpperCase()} />
    <VirtualList
      items={[{ path: "/a", size: 1 }]}
      key={(entry) => entry.path}
      renderRow={(entry, index) => <Text>{index()}: {entry.size.toFixed(0)}</Text>}
    />
  </View>
}

render(() => <App />)
