// The route editor overlay (wayfinder #40): the search pill, its suggestions/recents lists,
// and the collapsible route editor card (origin, ordered Waypoints -- ADR 0010 point 4 --,
// destination). Ported from prototype/planner-ui's VariantDGoogle.swift search pill/lists,
// adapted to the store's typed mutations (setDestination/addWaypoint/removeWaypoint/
// moveWaypoints) instead of the slice's single planTo(destination:).
import CoreLocation
import MapKit
import SwiftUI

struct RouteEditorView: View {
    let store: PlanStore
    /// Reports a user-facing message for RootView's shared toast (only "Search failed" today
    /// -- plan errors are reported by RootView itself via onChange(planErrorVersion)).
    var onToast: (String) -> Void

    @StateObject private var searchModel: DestinationSearchModel
    @FocusState private var searchFocused: Bool
    @State private var searchExpanded = false
    @State private var cardExpanded = true
    @State private var searchMode: SearchMode = .destination
    @AppStorage("recentDestinations") private var recentsRaw: String = "[]"

    /// M-07 (docs/codebase-audit-2026-08-29.md): seeds the search completer's bias from the
    /// store's active region at construction time, rather than a hardcoded corridor box --
    /// `onChange(of: store.activeRegion)` in `body` keeps it in sync afterwards.
    init(store: PlanStore, onToast: @escaping (String) -> Void) {
        self.store = store
        self.onToast = onToast
        _searchModel = StateObject(wrappedValue: DestinationSearchModel(region: store.activeRegion))
    }

    /// Whether a selected search result sets the destination or appends a Waypoint --
    /// entered by tapping the pill (destination) or the card's "+ Add stop" row (addStop).
    private enum SearchMode { case destination, addStop }

    private var recents: [RecentDestination] {
        (try? JSONDecoder().decode([RecentDestination].self, from: Data(recentsRaw.utf8))) ?? []
    }

    var body: some View {
        VStack(spacing: 8) {
            searchPill
            if searchExpanded {
                if !searchModel.query.isEmpty && !searchModel.results.isEmpty {
                    suggestionsList
                } else if searchModel.query.isEmpty && !recents.isEmpty {
                    recentsList
                }
            } else if store.destination != nil {
                routeEditorCard
            }
        }
        .onChange(of: store.activeRegion) { _, newRegion in
            searchModel.updateRegion(newRegion)
        }
    }

    // MARK: Search pill

    private var searchPill: some View {
        HStack(spacing: 10) {
            Image(systemName: "magnifyingglass")
                .foregroundColor(.secondary)
                // On the container this identifier would spread across every distinct
                // accessibility element inside (Image/TextField/Cancel button, depending on
                // expand state) instead of tagging one -- putting it on this always-present
                // icon keeps it a single, stable lookup target regardless of state.
                .accessibilityIdentifier("search-pill")
            if searchExpanded {
                TextField(searchMode == .addStop ? "Add stop" : "Search destination", text: $searchModel.query)
                    .focused($searchFocused)
                    .submitLabel(.search)
                if !searchModel.query.isEmpty {
                    Button {
                        searchModel.query = ""
                    } label: {
                        Image(systemName: "xmark.circle.fill").foregroundColor(.secondary)
                    }
                }
            } else {
                Text(store.destination?.name ?? "Search destination")
                    .foregroundColor(store.destination == nil ? .secondary : .primary)
                    .lineLimit(1)
            }
            Spacer()
            if store.isPlanning {
                ProgressView().scaleEffect(0.8)
            }
            if searchExpanded {
                Button("Cancel") { closeSearch() }
                    .font(.subheadline)
                    .foregroundColor(.blue)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .background(.regularMaterial, in: Capsule())
        .shadow(color: .black.opacity(0.15), radius: 6, y: 2)
        .contentShape(Capsule())
        .onTapGesture {
            guard !searchExpanded else { return }
            searchMode = .destination
            withAnimation { searchExpanded = true }
            searchFocused = true
        }
    }

    private var suggestionsList: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(searchModel.results.enumerated()), id: \.offset) { idx, completion in
                Button {
                    select(completion)
                } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(completion.title).font(.subheadline).foregroundColor(.primary)
                        if !completion.subtitle.isEmpty {
                            Text(completion.subtitle).font(.caption).foregroundColor(.secondary)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                }
                .buttonStyle(.plain)
                if idx != searchModel.results.count - 1 {
                    Divider().padding(.leading, 14)
                }
            }
        }
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .shadow(color: .black.opacity(0.15), radius: 6, y: 2)
    }

    private var recentsList: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(recents) { recent in
                Button {
                    selectRecent(recent)
                } label: {
                    HStack(spacing: 10) {
                        Image(systemName: "clock").foregroundColor(.secondary)
                        Text(recent.name).font(.subheadline).foregroundColor(.primary).lineLimit(1)
                        Spacer()
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("recent-\(recent.name)")
                if recent.id != recents.last?.id {
                    Divider().padding(.leading, 14)
                }
            }
        }
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .shadow(color: .black.opacity(0.15), radius: 6, y: 2)
    }

    private func select(_ completion: MKLocalSearchCompletion) {
        let search = MKLocalSearch(request: MKLocalSearch.Request(completion: completion))
        search.start { response, _ in
            Task { @MainActor in
                let mode = searchMode
                closeSearch()
                guard let coordinate = response?.mapItems.first?.placemark.coordinate else {
                    onToast("Search failed")
                    return
                }
                addRecent(name: completion.title, coordinate: coordinate)
                apply(mode: mode, name: completion.title, coordinate: coordinate)
            }
        }
    }

    private func selectRecent(_ recent: RecentDestination) {
        let mode = searchMode
        closeSearch()
        apply(mode: mode, name: recent.name, coordinate: recent.coordinate)
    }

    private func apply(mode: SearchMode, name: String, coordinate: CLLocationCoordinate2D) {
        switch mode {
        case .destination: store.setDestination(name: name, coordinate: coordinate)
        case .addStop: store.addWaypoint(name: name, coordinate: coordinate)
        }
    }

    private func closeSearch() {
        searchFocused = false
        withAnimation { searchExpanded = false }
        searchModel.query = ""
        searchMode = .destination
    }

    private func addRecent(name: String, coordinate: CLLocationCoordinate2D) {
        var list = recents.filter { $0.name != name }
        list.insert(RecentDestination(name: name, lat: coordinate.latitude, lon: coordinate.longitude), at: 0)
        list = Array(list.prefix(5))
        if let data = try? JSONEncoder().encode(list), let json = String(data: data, encoding: .utf8) {
            recentsRaw = json
        }
    }

    // MARK: Route editor card

    private var routeEditorCard: some View {
        VStack(spacing: 0) {
            Button {
                withAnimation { cardExpanded.toggle() }
            } label: {
                HStack {
                    Text(store.destination?.name ?? "Route").font(.subheadline).bold().lineLimit(1)
                    Spacer()
                    Image(systemName: cardExpanded ? "chevron.up" : "chevron.down")
                        .foregroundColor(.secondary)
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
            }
            .buttonStyle(.plain)

            if cardExpanded {
                Divider()
                List {
                    row(icon: "circle.fill", iconColor: .blue, title: store.originName, identifier: "route-origin-row")
                    ForEach(store.waypoints) { waypoint in
                        row(
                            icon: "smallcircle.filled.circle", iconColor: .orange, title: waypoint.name,
                            identifier: "route-waypoint-row-\(waypoint.name)"
                        )
                    }
                    // Native edit-mode delete controls, not a custom in-row xmark Button: the
                    // forced-active editMode below flattens each row into ONE accessibility
                    // element and absorbs taps on nested custom controls, which left the old
                    // xmark both dead to taps and invisible to VoiceOver/UI automation --
                    // caught by the first UI e2e run (PlanFlowTests.testWaypointAddRemove).
                    .onDelete { store.removeWaypoints(atOffsets: $0) }
                    .onMove { store.moveWaypoints(fromOffsets: $0, toOffset: $1) }
                    row(
                        icon: "mappin.circle.fill", iconColor: .red, title: store.destination?.name ?? "",
                        identifier: "route-destination-row"
                    )
                }
                .listStyle(.plain)
                .environment(\.editMode, .constant(.active))
                .scrollContentBackground(.hidden)
                .scrollDisabled(true)
                // List's own row height (not just this row's content padding) drives this --
                // measured empirically at ~65pt/row on the simulator.
                .frame(height: CGFloat(store.waypoints.count + 2) * 70)
                // OUTSIDE the List: the forced-active editMode above (needed for the Waypoint
                // rows' drag-reorder handles) makes the List absorb taps on nested custom
                // Buttons before their action runs, which left this row completely dead in the
                // UI -- caught by the first UI e2e run (PlanFlowTests.testWaypointAddRemove).
                // As an ordinary sibling button it is out of the edit-mode List's reach.
                Divider().padding(.leading, 14)
                addStopRow
            }
        }
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .shadow(color: .black.opacity(0.15), radius: 6, y: 2)
        // `.contain` keeps the card a real accessibility container: without it, identifying
        // this plain VStack swallows the identifiers of non-List children (the add-stop row,
        // now a direct sibling of the List), hiding them from UI automation.
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("route-editor-card")
    }

    /// One origin/Waypoint/destination row. Waypoint deletion is the ForEach's native
    /// edit-mode `.onDelete` control, not part of the row (see the comment there).
    private func row(icon: String, iconColor: Color, title: String, identifier: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: icon).font(.caption2).foregroundColor(iconColor)
            Text(title).font(.subheadline).lineLimit(1)
            Spacer()
        }
        .padding(.vertical, 4)
        .listRowBackground(Color.clear)
        .accessibilityIdentifier(identifier)
    }

    private var addStopRow: some View {
        Button {
            searchMode = .addStop
            withAnimation { searchExpanded = true }
            searchFocused = true
        } label: {
            HStack(spacing: 10) {
                Image(systemName: "plus.circle").foregroundColor(.blue)
                Text("Add stop").font(.subheadline).foregroundColor(.blue)
                Spacer()
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("add-stop-row")
    }
}
