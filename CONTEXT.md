# ABRP-native

A native iOS EV route planner replicating what is specific about ABRP (energy-aware routing with charging stops), built in Swift with Rust where performance demands it.

## Language

**Plan**:
The result of planning one journey: an ordered sequence of Legs and Charging Stops from origin to destination, with the predicted SoC curve.
_Avoid_: Route (that is a Leg's geometry), trip, itinerary

**Leg**:
A drive between two consecutive waypoints of a Plan (origin, Charging Stop or destination), with its road geometry, distance, duration and predicted energy use.
_Avoid_: Segment, section

**Charging Stop**:
A Charger chosen by the planner where the vehicle arrives at one SoC and leaves at a higher target SoC after a predicted charging duration.
_Avoid_: Charge, stop, station

**Waypoint**:
A user-chosen place the Plan must pass through, between origin and destination; distinct from a Charging Stop, which the planner chooses. A Waypoint that is a Charger may become a Charging Stop.
_Avoid_: Via, stop (UI copy may still say "Add stop"), intermediate destination

**Charger**:
A physical charging location with one or more connectors, a maximum power and an operator. The data object, independent of any Plan.
_Avoid_: Station, POI, pole

**SoC**:
State of charge, the battery level as a percentage of usable capacity.
_Avoid_: Battery %, charge level, range

**Vehicle Model**:
The set of parameters describing one car's energy behaviour: usable capacity, mass, drag, rolling resistance, drivetrain efficiency, charging curve. The first Vehicle Model is the Hyundai Ioniq 5 (2022).
_Avoid_: Car profile, consumption model (that is the Energy Model)

**Energy Model**:
The function that predicts a Leg's energy use from a Vehicle Model plus speed profile, elevation, temperature and wind.
_Avoid_: Consumption model, physics model

**Charging Curve**:
The vehicle's maximum accepted charging power as a function of SoC.
_Avoid_: Charge speed, charging profile

**Routing Engine**:
The component that computes a Leg's road geometry, distance and speed profile from the road graph.
_Avoid_: Router, navigation, directions

**Region Pack**:
A downloadable road graph covering one curated region, installed on the phone so the Routing Engine can compute Legs without connectivity. One Region Pack covers the whole trip region: cross-border Plans use a pack whose region spans the borders (packs are pre-merged in the pipeline, never stitched on the phone).
_Avoid_: Map download (that is map tiles), region, extract, country pack

**Map Pack**:
A downloadable set of map tiles covering one curated region, installed on the phone so the map can be drawn without connectivity. Its catalog mirrors the Region Pack's: one Map Pack covers the whole trip region, pre-merged in the pipeline, never stitched on the phone. Distinct from a Region Pack, which holds the road graph the Routing Engine uses.
_Avoid_: Offline map, tile bundle, map download

**Reference Consumption**:
The single user-facing calibration number of a Vehicle Model: the energy per km the car uses at a steady 110 km/h in mild conditions. Adjusting it scales the Energy Model's predictions; it does not replace the Energy Model.
_Avoid_: Efficiency, Wh/km setting, consumption factor

**Charger Pack**:
A downloadable set of Chargers covering one region, built from open national datasets and installed on the phone so the planner can choose Charging Stops without connectivity. Refreshed independently of the Region Pack.
_Avoid_: Charger database, POI file, station list

**Catalog**:
The hosted list of curated regions available to install, naming for each region the current version of its Region, Map and Charger Packs. The app reads it to offer installs and to detect that a refresh exists; installing a region means fetching the Packs the Catalog names for it.
_Avoid_: Manifest, region list, pack index, store

**SoC Curve**:
The predicted SoC along a Plan as a function of distance, sampled finely enough to chart, together with elevation and energy per km at each sample. The SoC/elevation chart is its rendering.
_Avoid_: Battery graph, consumption chart, path points

**Departure SoC**:
The SoC at the origin when the Plan starts.
_Avoid_: Initial SoC, start charge

**Destination Arrival SoC**:
The minimum SoC the Plan must have left when reaching the destination.
_Avoid_: Arrival buffer, reserve

**Charger Arrival SoC**:
The minimum SoC allowed on arriving at any Charging Stop or waypoint.
_Avoid_: Safety margin, min SoC

**Charger Max SoC**:
The highest SoC the planner will charge to at a Charging Stop before leaving.
_Avoid_: Charge limit, target SoC (that is the per-stop departure SoC the planner picks)

**Stops Bias**:
The user preference between few long Charging Stops, the quickest arrival, and many short Charging Stops.
_Avoid_: Charging strategy, stop mode

**Speed Cap**:
The maximum cruise speed the planner assumes for a Leg when lower than the road's own speeds, chosen so a Charging Stop can be skipped or reached; absent on an uncapped Leg.
_Avoid_: Speed adaptation, slow-down, eco speed

**Trip Log**:
The record of one real drive used to calibrate the Energy Model: an automatically captured GPS trace with timestamps and ambient temperature, plus the Display SoC read off the dash and entered by the driver at start and end.
_Avoid_: Drive log, telemetry, trip history

**Invalid Plan**:
A Plan the planner could not make satisfy the SoC constraints; it is still returned, with the failing Leg flagged, instead of an error.
_Avoid_: Failed route, no route

**Drive Mode**:
The state entered by Go in which the app follows the current Plan on the road: the position snapped to the route, the camera following, ETA and predicted SoC read at the live position, Charging Stops advanced as they are reached. Ends at arrival or by an explicit End.
_Avoid_: Navigation mode, turn-by-turn, guidance mode

**Go**:
The action that enters Drive Mode with the current Plan, available only when the Plan's origin is the current location; entering it also opens the drive's Trip Log.
_Avoid_: Start navigation, start route, depart

**Telemetry Profile**:
The data file describing how to read one vehicle's live telemetry over OBD: which ECUs answer, which identifiers to poll, how response bytes decode into signals, and the pack-variant constants. Data, not code — the polling and decoding engine is generic. Distinct from the Vehicle Model, which holds energy parameters; a supported car has both. Each profile carries a validation tier: car-validated (checked against a real car), vector-validated (passes recorded test vectors in the replay harness), or paper (defined, untested).
_Avoid_: PID list, decoder config, car module

**BMS SoC**:
The battery's true state of charge as its management system reports it, over the manufacturer-defined usable window. Read over OBD; diverges from Display SoC, most near full charge.
_Avoid_: Raw SoC, true SoC, real SoC

**Display SoC**:
The buffered remap of BMS SoC the car presents to the driver — the number on the dash, in the OEM app, and in a Trip Log's typed-in readings. What SoC means everywhere else in this glossary.
_Avoid_: Dash SoC, displayed charge
