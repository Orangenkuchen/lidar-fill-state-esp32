import * as THREE from "three";
import { GLTFLoader, type GLTF } from "three/addons/loaders/GLTFLoader.js";
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
// import { FirstPersonControls } from './FirstPersonControls.ts';
import STATS from 'three/addons/libs/stats.module.js';
import {
    Brush,
    Evaluator,
    INTERSECTION
} from "three-bvh-csg";


/**
 * A camera with controls
 */
interface CameraWithControls extends THREE.Camera
{
    /**
     * Show that the camera has controls
     */
    isCameraWithControls: boolean;
}

/**
 * A reading from the lidar
 */
interface LidarReading
{
    /**
     * The temperature of the Sensor
     */
    silicon_temp_degc: number;

    /**
     * The millisecond timestamp sice startup when the reading was done
     */
    last_read_timestamp_ms: number;

    /**
     * The distances that were measured (8 rows x 8 columns)
     */
    distance_mm: Array<number>;
}

interface DistanceMeasuremnt
{
    /**
     * The Distance from the floor
     */
    FromFloor: number | null;

    /**
     * The Distance from the Ceiling
     */
    FromCeiling: number | null;
}

class MainHandler
{
    // #region fields
    /**
     * The Window the canvas is in
     */
    private readonly window: Window;
    /**
     * The container of the canvas the 3D-Model should be rendered to
     */
    private readonly canvasContainer: HTMLElement;

    /**
     * The Canvas where the 3D-Model should be rendered to
     */
    private readonly canvas: HTMLCanvasElement;

    /**
     * The name of the mesh that acts as the main contianer that is filled
     */
    private readonly containerMeshName: string;

    /**
     * The name of the camera that acts as the position and fov of the lidar
     */
    private readonly lidarCameraName: string;

    /**
     * The main renderer
     */
    private readonly renderer: THREE.WebGLRenderer;

    /**
     * The main secene
     */
    private readonly mainScene: THREE.Scene;

    /**
     * The actions that should be called when cleaing up this class
     */
    private readonly cleanupActions: Array<() => void>;

    /**
     * Function with which the 3D-helper can be removed.
     */
    private readonly cleanupHelpers: Array<() => void>;

    /**
     * The cameras that can be used to display the 3d-model
     */
    private readonly cameras: Array<THREE.Camera>;
    /**
     * The registered Eventhandler for when the glb is loaded
     */
    private readonly glbLoadedEventHandlerList: Array<() => void>;

    /**
     * The registered Eventhandler for when the active camera changes
     */
    private readonly activeCameraChangeEventHandlerList: Array<() => void>;

    /**
     * The registered Eventhandler for when new data is received and displayed
     */
    private readonly newDataEventHandlerList: Array<() => void>;

    /**
     * The registered Eventhandler for when the loading of the glb is making progress
     */
    private readonly glbLoadingprogressEventHandlerList: Array<(loadingPercent: number) => void>;

    /**
     * The camera that is currently used to render the model
     */
    private activeCamera: THREE.Camera | null;

    /**
     * Hander for the display of statistics of the rendering
     */
    private stats: STATS | null;

    /**
     * Shows how much of the 3D-model is loaded. If null the model is loaded
     */
    private loadingPercent: number | null;

    /**
     * Controls to orbit the camera around a central point
     */
    private orbitControls: OrbitControls | null;

    /**
     * The Datetime when the last error was logged while running the game loop
     */
    private lastLoggedErrorInGameLoop: Date | null;

    /**
     * The main container mesh. Will only be filld after the glb is loaded.
     */
    private mainContainerMesh: THREE.Mesh | null;

    /**
     * The last temperature reading of the lidar. Is null at the beginning.
     */
    private lastSensorTemperature: number | null;

    /**
     * The last distance measurments to the ceiling and floor. Is null at the beginning.
     */
    private lastDistanceToContainer: DistanceMeasuremnt | null;

    // /**
    //  * Controls to control the camera like in a FPS game
    //  */
    // private firstPersonControls: FirstPersonControls | null;
    // #endregion

    // #region ctor
    /**
     * Initialises the Class
     * 
     * @param window The window the canvas is in
     * @param canvasContainerQuery The query-selector for the container of the canvas where the 3D-model should be drawn
     * @param containerMeshName The name of the mesh that acts as the main contianer that is filled
     * @param lidarCameraName The name of the camera that acts as the position and fov of the lidar
     */
    public constructor(
        window: Window,
        canvasContainerQuery: string,
        containerMeshName: string,
        lidarCameraName: string)
    {
        this.cleanupActions = new Array<() => void>();
        this.cleanupHelpers = new Array<() => void>();
        this.cameras = new Array<THREE.Camera>();
        this.activeCamera = null;
        this.stats = null;
        this.loadingPercent = 0;
        this.orbitControls = null;
        // this.firstPersionControls = null;
        this.lastLoggedErrorInGameLoop = null;
        this.glbLoadedEventHandlerList = new Array<() => void>();
        this.activeCameraChangeEventHandlerList = new Array<() => void>();
        this.glbLoadingprogressEventHandlerList = new Array<() => void>();
        this.newDataEventHandlerList = new Array<() => void>();
        this.mainContainerMesh = null;
        this.lastSensorTemperature = null;
        this.lastDistanceToContainer = null;

        this.window = window;
        this.containerMeshName = containerMeshName;
        this.lidarCameraName = lidarCameraName;

        let canvasContainer = document.querySelector<HTMLElement>(canvasContainerQuery);

        if (canvasContainer == null)
        {
            throw new Error(`The CanvasContainer "${canvasContainerQuery}" was not found.`);
        }

        this.canvasContainer = canvasContainer;

        let canvasList = this.canvasContainer.querySelectorAll("canvas");

        if (canvasList.length == 0)
        {
            throw new Error(`No Canvas in the CanvasContainer found.`);
        }
        else if (canvasList.length > 1)
        {
            console.warn(`Found ${canvasList.length} canvas in the CanvasContainer. Chossing the first and ignoring the rest...`);
        }

        let firstCanvas = canvasList[0];

        if (firstCanvas === undefined)
        {
            throw new Error("Unexcpected length while accessing the canvasList");
        }

        this.canvas = firstCanvas;

        this.renderer = new THREE.WebGLRenderer({ canvas: this.canvas, antialias: true, alpha: true });
        this.cleanupActions.push(() => {this.renderer.dispose();});
        this.mainScene = new THREE.Scene();
    }
    // #endregion
    
    // #region InitializeRender
    /**
     * Initialises the Renderer
     */
    public InitializeRender()
    {
        this.SetupStatsBox();

        this.renderer.toneMapping = THREE.ACESFilmicToneMapping;
        this.renderer.toneMappingExposure = 1;

        console.debug("3D-model: Adding a ambient light at %i%...", 45);
        let ambientLight = new THREE.AmbientLight(0xFFFFFF, 0.45);
        this.mainScene.add(ambientLight);

        let handleOnResize = () => { this.HandleOnResize(); };
        this.window.addEventListener("resize", handleOnResize);
        this.cleanupActions.push(() => { this.window.removeEventListener("resize", handleOnResize); });

        new GLTFLoader().load(
            "/api/model3d",
            (data) => this.HandleOnGlbLoadingIsDone(data),
            (progressEvent: ProgressEvent) => this.HandleOnGlbLoadProgress(progressEvent),
            (error) => {
                if (typeof (<any>error).response === "object" &&
                    (<any>error).response instanceof Response &&
                    (<Response>(<any>error).response).status == 474)
                {
                    console.info("3D-Model: Could not be loaded, because it's missing on the server.");
                    alert("Das 3D-Model ist noch nicht auf dem Server hinterlegt.\n\nDas Model kann in den Einstellungen hochgeladen werden.");
                }

                console.error("3D-Model: Error while loading the glb file: %o", error)
            }
        );

        console.log(this.lidarCameraName);
        this.HandleOnResize();
    }
    // #endregion

    // #region Add3dHelper
    /**
     * Adds 3D-Helper to the scene
     */
    public Add3dHelper()
    {
        console.debug("3D-Model: Adding grid helper...");
        let gridHelper = new THREE.GridHelper(10, 1);
        this.mainScene.add(gridHelper);
        this.cleanupHelpers.push(() => {this.mainScene.remove(gridHelper);gridHelper.dispose();});

        console.debug("3D-Model: Adding axis helper...");
        let axisHelper = new THREE.AxesHelper(2);
        this.mainScene.add(axisHelper);
        this.cleanupHelpers.push(() => {this.mainScene.remove(axisHelper);axisHelper.dispose();});

        for (let directionalLight of this.mainScene.getObjectsByProperty("isDirectionalLight", true))
        {
            let helper = new THREE.DirectionalLightHelper(<THREE.DirectionalLight>directionalLight);
            this.mainScene.add(helper);
        }

        for (let pointLight of this.mainScene.getObjectsByProperty("isPointLight", true))
        {
            let helper = new THREE.PointLightHelper(<THREE.PointLight>pointLight);
            this.mainScene.add(helper);
        }

        for (let spotLight of this.mainScene.getObjectsByProperty("isSpotLight", true))
        {
            let helper = new THREE.SpotLightHelper(<THREE.Light>spotLight);
            this.mainScene.add(helper);
        }

        for (let hemisphereLight of this.mainScene.getObjectsByProperty("isHemisphereLight ", true))
        {
            let helper = new THREE.HemisphereLightHelper(<THREE.HemisphereLight>hemisphereLight, 5);
            this.mainScene.add(helper);
        }

        for (let skinnedMesh of this.mainScene.getObjectsByProperty("isBone", true))
        {
            let helper = new THREE.SkeletonHelper(skinnedMesh);
            this.mainScene.add(helper);
        }
    }
    // #endregion

    // #region Remove3dHelpers
    /**
     * Removes the 3D-Helpers from the scene
     */
    public Remove3dHelpers()
    {
        for (let action of this.cleanupHelpers)
        {
            action();
        }
        this.cleanupHelpers.length = 0;
    }
    // #endregion

    // #region AddGlbLoadedEventHandler
    /**
     * Registers a handler for when the glb is loaded
     * 
     * @param handler The handler to register
     * @returns Returns a action to unsubscribe
     */
    public AddGlbLoadedEventHandler(handler: () => void): () => void
    {
        this.glbLoadedEventHandlerList.push(handler);

        return () => {
            let index = this.glbLoadedEventHandlerList.push(handler);

            if (index != -1)
            {
                this.glbLoadedEventHandlerList.splice(index);
            }
        };
    }
    // #endregion

    // #region AddActiveCameraChangeEventHandler
    /**
     * Registers a handler for when the active camera changes
     * 
     * @param handler The handler to register
     * @returns Returns a action to unsubscribe
     */
    public AddActiveCameraChangeEventHandler(handler: () => void): () => void
    {
        this.activeCameraChangeEventHandlerList.push(handler);

        return () => {
            let index = this.activeCameraChangeEventHandlerList.push(handler);

            if (index != -1)
            {
                this.activeCameraChangeEventHandlerList.splice(index);
            }
        };
    }
    // #endregion

    // #region AddGlbLoadingprogessEventHandler
    /**
     * Registers a handler for when the loading of the glb is making progress
     * 
     * @param handler The handler to register
     * @returns Returns a action to unsubscribe
     */
    public AddGlbLoadingprogessEventHandler(handler: (loadingPercent: number) => void): () => void
    {
        this.glbLoadingprogressEventHandlerList.push(handler);

        return () => {
            let index = this.glbLoadingprogressEventHandlerList.push(handler);

            if (index != -1)
            {
                this.glbLoadingprogressEventHandlerList.splice(index);
            }
        };
    }
    // #endregion

    // #region AddNewDataEventHandler
    /**
     * Registers a handler for when new data is received and displayed
     * 
     * @param handler The handler to register
     * @returns Returns a action to unsubscribe
     */
    public AddNewDataEventHandler(handler: () => void): () => void
    {
        this.newDataEventHandlerList.push(handler);

        return () => {
            let index = this.newDataEventHandlerList.push(handler);

            if (index != -1)
            {
                this.newDataEventHandlerList.splice(index);
            }
        };
    }
    // #endregion

    // #region ChangeCamera
    /**
     * Changes the Camera that is rendered
     * 
     * @param camera The camera that should be rended
     */
    public ChangeCamera(camera: THREE.Camera)
    {
        this.activeCamera = camera;
        this.UpdateCameraProjectionMatrixAndRatio(camera);
        this.SetupCameraControlsForSupporedCameras(camera);
    }
    // #endregion

    // #region CameraList
    /**
     * The cameras in the model
     */
    public get CameraList()
    {
        return this.cameras;
    }
    // #endregion

    // #region LastSensorTemperature
    /**
     * The last temperature reading of the lidar
     */
    public get LastSensorTemperature(): number | null
    {
        return this.lastSensorTemperature;
    }
    // #endregion

    // #region LastDistanceToContainer
    /**
     * The last distance measurments to the ceiling and floor. Is null at the beginning.
     */
    public get LastDistanceToContainer(): DistanceMeasuremnt | null
    {
        return this.lastDistanceToContainer;
    }
    // #endregion

    // #region LogActiveCameraPosition
    /**
     * Logs the position of the active camera in the console
     */
    public LogActiveCameraPosition()
    {
        if (this.activeCamera == null)
        {
            console.info("3D-Model: Active Camera is null.");
        }
        else
        {
            console.info(
                "3D-Model: Active Camera-Position: x=%f; y=%f; z=%f; Camera-Rotations: x=%f; y=%f; z=%f;",
                this.activeCamera.position.x,
                this.activeCamera.position.y,
                this.activeCamera.position.z,
                this.activeCamera.rotation.x,
                this.activeCamera.rotation.y,
                this.activeCamera.rotation.z
            );
        }
    }
    // #endregion

    // #region CalculateMainContainerVolume
    /**
     * Calculates the Volume of the MainContainer
     * 
     * @returns Returns the Volume
     */
    public CalculateMainContainerVolume(): number
    {
        if (this.mainContainerMesh == null)
        {
            return 0;
        }

        return this.CalculateVolumeOfMesh(this.mainContainerMesh);
    }
    // #endregion

    // #region CalculateVolumeOfMesh
    /**
     * Calcaluates the volume of the mesh
     * 
     * @param mesh The mesh to calculate
     * @returns Returns the volume
     */
    private CalculateVolumeOfMesh(mesh: THREE.Mesh): number
    {
        const geometry = mesh.geometry;
        const position = geometry.attributes["position"];
        const index = geometry.index;

        if (position === undefined)
        {
            return 0;
        }

        let volume = 0;

        const a = new THREE.Vector3();
        const b = new THREE.Vector3();
        const c = new THREE.Vector3();

        const triangleCount = index
            ? index.count / 3
            : position.count / 3;

        for (let i = 0; i < triangleCount; i++) {
            const i0 = index ? index.getX(i * 3) : i * 3;
            const i1 = index ? index.getX(i * 3 + 1) : i * 3 + 1;
            const i2 = index ? index.getX(i * 3 + 2) : i * 3 + 2;

            a.fromBufferAttribute(position, i0);
            b.fromBufferAttribute(position, i1);
            c.fromBufferAttribute(position, i2);

            volume += a.dot(b.clone().cross(c)) / 6;
        }

        return Math.abs(volume);
    }
    // #endregion

    // #region RaiseGlbLoaded
    /**
     * Raises the event for when the glb is loaded
     */
    private RaiseGlbLoaded(): void
    {
        for (let handler of this.glbLoadedEventHandlerList)
        {
            handler();
        }
    }
    // #endregion

    // #region RaiseActiveCameraChange
    /**
     * Raises the event for when the active camera changes
     */
    private RaiseActiveCameraChange(): void
    {
        for (let handler of this.activeCameraChangeEventHandlerList)
        {
            handler();
        }
    }
    // #endregion

    // #region RaiseGlbLoadingprogess
    /**
     * Raises the event for when the loading of the glb is making progress
     * 
     * @param loadingPercent The percent the glb is loaded
     */
    private RaiseGlbLoadingprogess(loadingPercent: number): void
    {
        for (let handler of this.glbLoadingprogressEventHandlerList)
        {
            handler(loadingPercent);
        }
    }
    // #endregion

    // #region RaiseNewData
    /**
     * Raises the event for when new data is received and displayed
     */
    private RaiseNewData(): void
    {
        for (let handler of this.newDataEventHandlerList)
        {
            handler();
        }
    }
    // #endregion

    // #region HandleOnResize
    /**
     * Is called when the size of the window changes.
     * It then sets the size and apsect ratio of the renderer and canvs.
     */
    private HandleOnResize()
    {
        this.SetRatioAndSizeInTheRenderer(
            this.window,
            this.canvasContainer.clientWidth,
            this.canvasContainer.clientHeight
        );

        if (this.activeCamera != null)
        {
            this.UpdateCameraProjectionMatrixAndRatio(this.activeCamera);
        }
    }
    // #endregion

    // #region HandleOnGlbLoadingIsDone
    /**
     * Is called when the glb is successfully loaded. Sets the rendering of it
     * up then and starts the game loop
     *
     * @param loadedGLB The glb that was loaded
     */
    private HandleOnGlbLoadingIsDone(loadedGLB: GLTF)
    {
        this.loadingPercent = null;

        console.debug("3D-Model: Glb was loaded. Adding it to scene...");
        this.mainScene.add(loadedGLB.scene);

        let mainContainerMesh = <THREE.Mesh>loadedGLB.scene.getObjectByName(this.containerMeshName);
        if (mainContainerMesh !== undefined) {
            this.mainContainerMesh = mainContainerMesh;
        }

        let center: THREE.Vector3;
        if (mainContainerMesh === null)
        {
            console.error(
                "3D-Model: The mesh of the container that is filled wan't found under the name \"%s\". Display will not work correctly.",
                this.containerMeshName
            );
            center = new THREE.Vector3(0, 0, 0);
        }
        else
        {
            const box = new THREE.Box3().setFromObject(mainContainerMesh);
            center = box.getCenter(new THREE.Vector3());
        }

        this.cameras.length = 0;

        let perspectiveCamera = new THREE.PerspectiveCamera(75, this.GetAspectRatio(), 0.1, 2000);
        perspectiveCamera.name = "Haupt-Kamera";
        perspectiveCamera.position.set(-0.345346, 2.500441, -3.571497);
        perspectiveCamera.lookAt(center);
        perspectiveCamera.updateProjectionMatrix();
        (<CameraWithControls><unknown>perspectiveCamera).isCameraWithControls = true;
        this.mainScene.add(perspectiveCamera);
        this.cameras.push(perspectiveCamera);
        this.activeCamera = perspectiveCamera;

        let orthographicCamera = new THREE.OrthographicCamera();
        orthographicCamera.name = "Orthogonal-Kamera";
        orthographicCamera.position.set(-1, 3, -1);
        orthographicCamera.lookAt(center);
        orthographicCamera.updateProjectionMatrix();
        (<CameraWithControls><unknown>orthographicCamera).isCameraWithControls = true;
        this.mainScene.add(orthographicCamera);
        this.cameras.push(orthographicCamera);

        this.UpdateCameraProjectionMatrixAndRatio(this.activeCamera);
        this.SetupCameraControlsForSupporedCameras(this.activeCamera);

        // Changing the MainContainer Mesh to only render the front side (which should face inwards)
        const materials = Array.isArray(mainContainerMesh.material)
            ? mainContainerMesh.material
            : [mainContainerMesh.material];
        for (const material of materials) {
            material.side = THREE.BackSide;
            material.needsUpdate = true;
        }

        this.GameLoop();
        this.RaiseGlbLoaded();
        this.RaiseActiveCameraChange();

        this.SetupFillStateMesh();
    }
    // #endregion

    // #region UpdateCameraProjectionMatrixAndRatio
    /**
     * Updates the cameras projection matrix and the aspect ratio
     * 
     * @param camera The camera to update
     */
    private UpdateCameraProjectionMatrixAndRatio(camera: THREE.Camera): void
    {
        console.debug("3D-Model: Updating the matrix/ratio of the camera...");

        let aspectRatio = this.GetAspectRatio();

        if ((<THREE.PerspectiveCamera>camera).isPerspectiveCamera == true)
        {
            let castedCamera = <THREE.PerspectiveCamera>camera;

            console.debug("3D-Model: Setting the aspect ratio of %f to the perspective camera...", aspectRatio);
            castedCamera.aspect = aspectRatio;
            castedCamera.updateProjectionMatrix();
        }
        else if ((<THREE.OrthographicCamera>camera).isOrthographicCamera == true)
        {
            let castedCamera = <THREE.OrthographicCamera>camera;

            let heightOfCamera = castedCamera.top - castedCamera.bottom;
            let centerX = (castedCamera.left + castedCamera.right) / 2;

            // Change the width, height of the camera to the aspect ratio but
            // keep the center where it is
            let newWidth = heightOfCamera * aspectRatio;
            castedCamera.right = centerX + (newWidth / 2);
            castedCamera.left = centerX - (newWidth / 2);

            console.debug("3D-Model: The new camera-size is %fx%f (aspectRatio: %f).", newWidth, heightOfCamera, aspectRatio);
        }
        else
        {
            console.warn("The camera-type is unknown (%o). Can't update ratio.", camera);
        }
    }
    // #endregion

    // #region GetAspectRatio
    /**
     * Gets the aspect ratio for the canvas
     * 
     * @returns Returns the ratio
     */
    private GetAspectRatio()
    {
        return this.canvasContainer.clientWidth / this.canvasContainer.clientHeight;
    }
    // #endregion

    // #region SetRatioAndSizeInTheRenderer
    /**
     * Sets the aspect ratio and the dimensions of the renderer.
     * Also sets the devicePixelRatio in the render.
     * 
     * @param window The window in which the renderer is
     * @param renderWidth The width of the rendered image (not how big it should be)
     * @param renderHeight The height of the rendered image (not how big it should be)
     */
    private SetRatioAndSizeInTheRenderer(window: Window, renderWidth: number, renderHeight: number): void
    {
        this.canvas.removeAttribute("style");
        this.canvas.setAttribute("height", renderHeight.toString());
        this.canvas.setAttribute("width", renderWidth.toString());

        console.debug("3D-Model: Setting the devicePixelRatio to %f...", window.devicePixelRatio);
        this.renderer.setPixelRatio(window.devicePixelRatio);
        console.debug("3D-Model: Changing the size of the renderer to %i x %i", renderWidth, renderHeight);
        this.renderer.setSize(renderWidth, renderHeight);
    }
    // #endregion

    // #region SetupStatsBox
    /**
     * Sets the stats box up to display the render stats
     */
    private SetupStatsBox()
    {
        this.stats = new STATS();
        this.stats.showPanel(0)

        this.canvasContainer.appendChild(this.stats.dom);

        // Move the stats-box to be in the top left corner of the container-element
        let containerBoundingBox = this.canvasContainer.getBoundingClientRect();
        
        this.stats.dom.style.left = `${containerBoundingBox.x}px`;
        this.stats.dom.style.top = `${containerBoundingBox.y}px`;
    }
    // #endregion

    // #region SetupCameraControlsForSupporedCameras
    /**
     * Sets camera controls up for the camera if it supports it
     *
     * @param camera The camera to setup the controls for
     */
    private SetupCameraControlsForSupporedCameras(camera: THREE.Camera)
    {
        this.orbitControls?.dispose();
        this.orbitControls = null;

        // this.firstPersonControls?.dispose();
        // this.firstPersonControls = null;

        if ((<CameraWithControls><unknown>camera).isCameraWithControls !== undefined &&
            (<CameraWithControls><unknown>camera).isCameraWithControls)
        {
            this.orbitControls = new OrbitControls(camera, this.renderer.domElement);
            
            if (this.mainContainerMesh != null)
            {
                const box = new THREE.Box3().setFromObject(this.mainContainerMesh);
                let center = box.getCenter(new THREE.Vector3());
                center.y -= 0.5;
                this.orbitControls.target = center;
            }

            // this.firstPersonControls = new FirstPersonControls(camera, this.renderer.domElement);
            // this.firstPersonControls.Enabled = false;
            // this.firstPersonControls.MouseLookSpeed = 15;
            // this.firstPersonControls.MovementSpeed = 15;
            // this.firstPersonControls.ControllerLookSpeed = 50;
        }
    }
    // #endregion

    // #region HandleOnGlbLoadProgress
    /**
     * Is Called when the GLB-Loading is making progress
     * 
     * @param event The loading event
     */
    private HandleOnGlbLoadProgress(event: ProgressEvent)
    {
        let progress = Math.round(event.loaded / event.total * 100);

        console.debug("3D-Model: GLTF loading at %f%", progress);
        this.loadingPercent = progress;

        this.RaiseGlbLoadingprogess(this.loadingPercent);
    }
    // #endregion

    // #region GameLoop
    /**
     * Starts the Gameloop in which the model is rendered to the canvas
     */
    private GameLoop(): void
    {
        try
        {
            if (this.activeCamera != null)
            {
                this.stats?.begin();
                if (this.orbitControls?.enabled)
                {
                    this.orbitControls.update();
                }
                // if (this.firstPersonControls?.enabled)
                // {
                //     this.firstPersonControls.update();
                // }

                this.renderer.render(this.mainScene, this.activeCamera);

                this.stats?.end();
            }
        }
        catch(error: any)
        {
            if (this.lastLoggedErrorInGameLoop == null ||
                new Date().getTime() - this.lastLoggedErrorInGameLoop.getTime() > 2000)
            {
                console.error("3D-Model: Error while running the gameloop: %o", error);
                this.lastLoggedErrorInGameLoop = new Date();
            }
        }
        finally
        {
            requestAnimationFrame(() => { this.GameLoop(); });
        }
    }
    // #endregion

    // #region SetupFillStateMesh
    /**
     * Sets up the periodic fetching of the lidar reading and
     * creates a mesh from this to represent the fill state in the container
     */
    private SetupFillStateMesh(): void
    {
        let lidar = this.mainScene.getObjectByName(this.lidarCameraName);

        if (lidar === undefined)
        {
            console.error(
                "3D-Model: Lidar-Camera with the name \"%s\" wan't found. Fillstate will not be shown.",
                this.lidarCameraName
            );
            return;
        }
        else if (typeof (<THREE.PerspectiveCamera>lidar).isPerspectiveCamera !== "boolean")
        {
            console.error(
                "3D-Model: Lidar-Camera was found but isn't a perspective camera. Fillstate will not be shown.",
                this.lidarCameraName
            );
            return;
        }

        let perspectiveCameraLidar = <THREE.PerspectiveCamera>lidar;

        let verticalFov = perspectiveCameraLidar.fov;
        let horizontalFov = verticalFov * ((<any>perspectiveCameraLidar).aspectRatio ?? 1);

        let lastIntersectMeshDebug: THREE.Mesh | null = null;
        let getReadingAndDisplayInMesh = async () =>
        {
            try
            {
                let reading = await this.FetchLidarReading();

                if (reading == null)
                {
                    console.warn("3D-Model: Lidar reading was null. Skipping...");
                    return;
                }

                this.lastSensorTemperature = reading.silicon_temp_degc;

                let newMesh = this.CreateLidarMesh(8, 8, verticalFov, horizontalFov, reading!.distance_mm, 10, 5);

                newMesh.updateWorldMatrix(true, false);
                this.mainContainerMesh!.updateWorldMatrix(true, false);

                lidar.add(newMesh);

                let intersectMesh = this.IntersectMeshes(newMesh, this.mainContainerMesh!);
                lidar.remove(newMesh);
                if (lastIntersectMeshDebug != null)
                {
                    this.mainScene.remove(lastIntersectMeshDebug);
                    lastIntersectMeshDebug = null;
                }
                this.mainScene.add(intersectMesh);
                lastIntersectMeshDebug = intersectMesh;

                this.lastDistanceToContainer = this.GetDistanceFromCeilingFloor(newMesh, this.mainContainerMesh!);

                this.RaiseNewData();
            }
            finally
            {
                setTimeout(getReadingAndDisplayInMesh, 2000);
            }
        };

        setTimeout(getReadingAndDisplayInMesh, 2000);
        getReadingAndDisplayInMesh();
    }
    // #endregion

    // #region CreateLidarMesh
    /**
     * Creates a mesh for the LiDAR reading, including an outer border.
     *
     * @param rows The amount of LiDAR measurements per row
     * @param columns The amount of LiDAR measurements per column
     * @param fovX_in_deg The FOV in x direction in degrees
     * @param fovY_in_deg The FOV in y direction in degrees
     * @param readings_in_mm The array with the readings in mm
     * @param borderWidth The border extension width in meters
     *
     * @returns The created mesh
     */
    private CreateLidarMesh(
        rows: number,
        columns: number,
        fovX_in_deg: number,
        fovY_in_deg: number,
        readings_in_mm: Array<number>,
        borderWidth: number = 0.5,
        extrusionDepth: number = 1.0): THREE.Mesh
    {
        const fovX = THREE.MathUtils.degToRad(fovX_in_deg);
        const fovY = THREE.MathUtils.degToRad(fovY_in_deg);

        const positions: number[] = [];
        const indices: number[] = [];
        const uvs: number[] = [];

        // Create the LiDAR grid vertices.
        for (let row = 0; row < rows; row++) {
            for (let col = 0; col < columns; col++) {
                const theta = ((col + 0.5) / columns - 0.5) * fovX;
                const phi = (0.5 - (row + 0.5) / rows) * fovY;

                const d = readings_in_mm[row * columns + col]! / 1000;

                // Treat d as forward depth.
                const x = d * Math.tan(theta);
                const y = d * Math.tan(phi);
                const z = -d;

                positions.push(x, y, z);

                const u = columns > 1 ? col / (columns - 1) : 0.5;
                const v = rows > 1 ? 1 - row / (rows - 1) : 0.5;

                uvs.push(u, v);
            }
        }

        // Connect the LiDAR grid cells.
        for (let row = 0; row < rows - 1; row++) {
            for (let col = 0; col < columns - 1; col++) {
                const a = row * columns + col;
                const b = a + 1;
                const c = a + columns;
                const d = c + 1;

                indices.push(a, c, b);
                indices.push(b, c, d);
            }
        }

        // Add the outer border.
        if (borderWidth > 0 && rows >= 2 && columns >= 2) {
            // Find the center of the grid in the XY plane.
            let minX = Infinity;
            let maxX = -Infinity;
            let minY = Infinity;
            let maxY = -Infinity;

            for (let i = 0; i < positions.length; i += 3) {
                minX = Math.min(minX, positions[i]!);
                maxX = Math.max(maxX, positions[i]!);
                minY = Math.min(minY, positions[i + 1]!);
                maxY = Math.max(maxY, positions[i + 1]!);
            }

            const centerX = (minX + maxX) / 2;
            const centerY = (minY + maxY) / 2;

            // Collect the perimeter vertices in order.
            const perimeter: number[] = [];

            // First row.
            for (let col = 0; col < columns; col++) {
                perimeter.push(col);
            }

            // Right edge.
            for (let row = 1; row < rows; row++) {
                perimeter.push(row * columns + columns - 1);
            }

            // Last row.
            for (let col = columns - 2; col >= 0; col--) {
                perimeter.push((rows - 1) * columns + col);
            }

            // Left edge.
            for (let row = rows - 2; row > 0; row--) {
                perimeter.push(row * columns);
            }

            const outerPerimeter: number[] = [];

            // Create an outward offset vertex for each perimeter vertex.
            for (const index of perimeter) {
                const i = index * 3;

                const x = positions[i]!;
                const y = positions[i + 1]!;
                const z = positions[i + 2]!;

                const dx = x - centerX;
                const dy = y - centerY;
                const length = Math.sqrt(dx * dx + dy * dy);

                const nx = length > 1e-8 ? dx / length : 0;
                const ny = length > 1e-8 ? dy / length : 0;

                const outerIndex = positions.length / 3;

                positions.push(
                    x + nx * borderWidth,
                    y + ny * borderWidth,
                    z
                );

                // Extend the texture UVs to the border.
                uvs.push(
                    uvs[index * 2]!,
                    uvs[index * 2 + 1]!
                );

                outerPerimeter.push(outerIndex);
            }

            // Connect the original perimeter to the outer perimeter.
            for (let i = 0; i < perimeter.length; i++) {
                const next = (i + 1) % perimeter.length;

                const innerA = perimeter[i]!;
                const innerB = perimeter[next]!;
                const outerA = outerPerimeter[i]!;
                const outerB = outerPerimeter[next]!;

                indices.push(
                    innerA, innerB, outerA,
                    innerB, outerB, outerA
                );
            }
        }

        // Extrude the entire mesh downwards.
        if (extrusionDepth > 0) {
            const originalVertexCount = positions.length / 3;
            const originalIndexCount = indices.length;

            // Duplicate every vertex and move it downwards.
            for (let i = 0; i < originalVertexCount; i++) {
                const x = positions[i * 3]!;
                const y = positions[i * 3 + 1]!;
                const z = positions[i * 3 + 2]!;

                positions.push(x, y, z - extrusionDepth);

                // Copy UV coordinates.
                uvs.push(
                    uvs[i * 2]!,
                    uvs[i * 2 + 1]!
                );
            }

            // Duplicate the existing triangles for the bottom face.
            // Reverse winding so the bottom faces point in the opposite direction.
            for (let i = 0; i < originalIndexCount; i += 3) {
                const a = indices[i]!;
                const b = indices[i + 1]!;
                const c = indices[i + 2]!;

                indices.push(
                    a + originalVertexCount,
                    c + originalVertexCount,
                    b + originalVertexCount
                );
            }

            // Connect the perimeter to the duplicated perimeter.
            if (rows >= 2 && columns >= 2) {
                const perimeter: number[] = [];

                // First row.
                for (let col = 0; col < columns; col++) {
                    perimeter.push(col);
                }

                // Right edge.
                for (let row = 1; row < rows; row++) {
                    perimeter.push(row * columns + columns - 1);
                }

                // Last row.
                for (let col = columns - 2; col >= 0; col--) {
                    perimeter.push((rows - 1) * columns + col);
                }

                // Left edge.
                for (let row = rows - 2; row > 0; row--) {
                    perimeter.push(row * columns);
                }

                // Include the outer border vertices in the perimeter.
                // These are the vertices added by your border-generation code.
                const borderStart = rows * columns;

                const outerPerimeter: number[] = [];

                for (let i = 0; i < perimeter.length; i++) {
                    outerPerimeter.push(borderStart + i);
                }

                // The actual outer boundary is the outer perimeter,
                // not the original LiDAR perimeter.
                const boundary = outerPerimeter;

                // Create side walls along the outer boundary.
                for (let i = 0; i < boundary.length; i++) {
                    const next = (i + 1) % boundary.length;

                    const a = boundary[i]!;
                    const b = boundary[next]!;

                    const bottomA = a + originalVertexCount;
                    const bottomB = b + originalVertexCount;

                    indices.push(
                        a, b, bottomA,
                        b, bottomB, bottomA
                    );
                }
            }
        }

        // Create the geometry.
        const geometry = new THREE.BufferGeometry();

        geometry.setAttribute(
            "position",
            new THREE.Float32BufferAttribute(positions, 3)
        );

        geometry.setAttribute(
            "uv",
            new THREE.Float32BufferAttribute(uvs, 2)
        );

        geometry.setIndex(indices);
        geometry.computeVertexNormals();

        // Get the material.
        let material: THREE.MeshStandardMaterial;

        const contentMaterial = this.mainScene.getObjectByName(
            "ContentMaterial_Helper_Mesh"
        );

        const candidate = contentMaterial as THREE.Mesh | undefined;
        const existingMaterial = candidate?.material;

        if (
            existingMaterial &&
            !Array.isArray(existingMaterial) &&
            (existingMaterial as THREE.Material).type === "MeshStandardMaterial"
        ) {
            material = existingMaterial as THREE.MeshStandardMaterial;
        } else {
            material = new THREE.MeshStandardMaterial({
                color: 0x00ff88,
                side: THREE.DoubleSide,
                wireframe: false,
                transparent: true,
                opacity: 0.65
            });
        }

        if (material.map) {
            material.map.wrapS = THREE.RepeatWrapping;
            material.map.wrapT = THREE.RepeatWrapping;
            material.map.repeat.set(columns, rows);
            material.map.needsUpdate = true;
        }

        return new THREE.Mesh(geometry, material);
    }
    // #endregion

    private IntersectMeshes(
        lidarMesh: THREE.Mesh,
        containerMesh: THREE.Mesh
    ): THREE.Mesh {
        lidarMesh.updateWorldMatrix(true, false);
        containerMesh.updateWorldMatrix(true, false);
        this.mainScene.updateWorldMatrix(true, false);

        const createWorldGeometry = (
            mesh: THREE.Mesh,
            name: string
        ): THREE.BufferGeometry => {
            const geometry = mesh.geometry.clone();
            geometry.name = name;

            // Bake the complete mesh world transform into the geometry.
            geometry.applyMatrix4(mesh.matrixWorld);

            // Ensure triangles are explicitly indexed.
            if (!geometry.index) {
                const position = geometry.getAttribute("position");
                const indices: number[] = [];

                for (let i = 0; i + 2 < position.count; i += 3) {
                    indices.push(i, i + 1, i + 2);
                }

                geometry.setIndex(indices);
            }

            geometry.deleteAttribute("normal");
            geometry.computeVertexNormals();
            geometry.computeBoundingBox();
            geometry.computeBoundingSphere();

            return geometry;
        };

        const calculateSignedVolume = (
            geometry: THREE.BufferGeometry
        ): number => {
            const position = geometry.getAttribute("position");
            const index = geometry.index;

            if (!position || !index) {
                return 0;
            }

            const a = new THREE.Vector3();
            const b = new THREE.Vector3();
            const c = new THREE.Vector3();
            const cross = new THREE.Vector3();

            let volume = 0;

            for (let i = 0; i + 2 < index.count; i += 3) {
                a.fromBufferAttribute(position, index.getX(i));
                b.fromBufferAttribute(position, index.getX(i + 1));
                c.fromBufferAttribute(position, index.getX(i + 2));

                volume += a.dot(cross.crossVectors(b, c)) / 6;
            }

            return volume;
        };

        const reverseTriangleWinding = (
            geometry: THREE.BufferGeometry
        ): void => {
            const index = geometry.index;

            if (!index) {
                return;
            }

            for (let i = 0; i + 2 < index.count; i += 3) {
                const second = index.getX(i + 1);
                const third = index.getX(i + 2);

                index.setX(i + 1, third);
                index.setX(i + 2, second);
            }

            index.needsUpdate = true;

            geometry.deleteAttribute("normal");
            geometry.computeVertexNormals();
            geometry.computeBoundingBox();
            geometry.computeBoundingSphere();
        };

        const lidarGeometry = createWorldGeometry(
            lidarMesh,
            "LIDAR"
        );

        const containerGeometry = createWorldGeometry(
            containerMesh,
            "CONTAINER"
        );

        const lidarVolume = calculateSignedVolume(lidarGeometry);
        const containerVolume = calculateSignedVolume(containerGeometry);

        // Reverse inward-facing closed meshes.
        if (lidarVolume < 0) {
            reverseTriangleWinding(lidarGeometry);
        }

        if (containerVolume < 0) {
            reverseTriangleWinding(containerGeometry);
        }

        // The brushes now contain world-space vertices.
        // Keep their transforms at identity to avoid applying transforms twice.
        const material = new THREE.MeshStandardMaterial({
            color: 0x00ff88,
            side: THREE.DoubleSide
        });

        const lidarBrush = new Brush(lidarGeometry, material);
        const containerBrush = new Brush(containerGeometry, material);

        lidarBrush.position.set(0, 0, 0);
        lidarBrush.rotation.set(0, 0, 0);
        lidarBrush.scale.set(1, 1, 1);
        lidarBrush.updateMatrix();
        lidarBrush.updateMatrixWorld(true);

        containerBrush.position.set(0, 0, 0);
        containerBrush.rotation.set(0, 0, 0);
        containerBrush.scale.set(1, 1, 1);
        containerBrush.updateMatrix();
        containerBrush.updateMatrixWorld(true);

        const evaluator = new Evaluator();

        const result = evaluator.evaluate(
            containerBrush,
            lidarBrush,
            INTERSECTION
        );

        const resultGeometry = result.geometry;

        resultGeometry.computeBoundingBox();
        resultGeometry.computeBoundingSphere();

        const resultBounds = resultGeometry.boundingBox;

        if (
            !resultBounds ||
            !Number.isFinite(resultBounds.min.x) ||
            resultGeometry.getAttribute("position").count === 0
        ) {
            lidarGeometry.dispose();
            containerGeometry.dispose();
            material.dispose();

            throw new Error(
                "CSG intersection returned empty or invalid geometry."
            );
        }

        // Convert world-space intersection vertices into scene-local space.
        const inverseSceneWorld = this.mainScene.matrixWorld
            .clone()
            .invert();

        resultGeometry.applyMatrix4(inverseSceneWorld);
        resultGeometry.deleteAttribute("normal");
        resultGeometry.computeVertexNormals();
        resultGeometry.computeBoundingBox();
        resultGeometry.computeBoundingSphere();










        let material2: THREE.MeshStandardMaterial;
    
        const contentMaterial = this.mainScene.getObjectByName(
            "ContentMaterial_Helper_Mesh"
        );

        const candidate = contentMaterial as THREE.Mesh | undefined;
        const existingMaterial = candidate?.material;

        if (
            existingMaterial &&
            !Array.isArray(existingMaterial) &&
            (existingMaterial as THREE.Material).type === "MeshStandardMaterial"
        ) {
            material2 = existingMaterial as THREE.MeshStandardMaterial;
        } else {
            material2 = new THREE.MeshStandardMaterial({
                color: 0x00ff88,
                side: THREE.DoubleSide,
                wireframe: false,
                transparent: true,
                opacity: 0.65
            });
        }

        // if (material2.map) {
        //     material2.map.wrapS = THREE.RepeatWrapping;
        //     material2.map.wrapT = THREE.RepeatWrapping;
        //     material2.map.repeat.set(columns, rows);
        //     material2.map.needsUpdate = true;
        // }

        result.material = material2;

        result.material.needsUpdate = true;

        result.position.set(0, 0, 0);
        result.quaternion.identity();
        result.scale.set(1, 1, 1);
        result.updateMatrix();
        result.updateMatrixWorld(true);

        lidarGeometry.dispose();
        containerGeometry.dispose();
        material.dispose();

        return result;
    }

    // #region GetDistanceFromCeilingFloor
    /**
     * Gets the distances from the object to the floor and ceiling of the container
     * 
     * @param object The object from which to measure
     * @param container The container to measure to
     * @returns Returns the values. The values are null if they never hit the container
     */
    private GetDistanceFromCeilingFloor(object: THREE.Object3D, container: THREE.Mesh): DistanceMeasuremnt
    {
        let measurementToCeiling = this.GetDistanceInDirection(object, container, new THREE.Vector3(0, 1, 0))
        let measurementToFloor = this.GetDistanceInDirection(object, container, new THREE.Vector3(0, -1, 0))

        return { FromCeiling: measurementToCeiling, FromFloor: measurementToFloor }
    }
    // #endregion

    // #region GetDistanceInDirection
    /**
     * Gets the distance between the object and the next intersection with the floorMesh
     * 
     * @param object The object from which to start
     * @param floorMesh The object to intersect with
     * @param direction The direction of the measurement from "object"
     * @returns Returns the distance to the intersection or null if it never intersects
     */
    private GetDistanceInDirection(object: THREE.Object3D, floorMesh: THREE.Mesh, direction: THREE.Vector3)
    {
        object.updateWorldMatrix(true, false);
        floorMesh.updateWorldMatrix(true, false);

        const origin = this.GetMeshCenterWorld(<any>object);

        const normalizedDirection = direction.clone().normalize();

        const raycaster = new THREE.Raycaster(
            origin,
            normalizedDirection,
            0,
            Infinity
        );

        const materials = Array.isArray(floorMesh.material)
            ? floorMesh.material
            : [floorMesh.material];

        const originalSides = materials.map(material => material.side);

        try
        {
            // Make the material doublesided so that the ray always hits
            // will be reset afterwards
            for (const material of materials)
            {
                material.side = THREE.DoubleSide;
            }

            const hits = raycaster.intersectObject(floorMesh, true);

            if (hits.length < 1) {
                return null; // No floor intersection
            }

            return hits[0]!.distance;
        }
        finally
        {
            materials.forEach((material, i) => {
                material.side = originalSides[i]!;
            });
        }
    }
    // #endregion

    // #region GetMeshCenterWorld
    /**
     * Gets the Center of the Mesh-Points (not the Mesh-Origin)
     * 
     * @param mesh The Mesh to use
     * @returns Returns the Position
     */
    private GetMeshCenterWorld(mesh: THREE.Mesh): THREE.Vector3 {
        mesh.updateWorldMatrix(true, false);

        const geometry = mesh.geometry;
        geometry.computeBoundingBox();

        // Center of the geometry's local-space bounding box
        const center = geometry.boundingBox!.getCenter(new THREE.Vector3());

        // Convert the local-space point to world coordinates
        return mesh.localToWorld(center);
    }
    // #endregion

    // #region FetchLidarReading
    /**
     * Fetches the Lidar-Reading from the api
     * 
     * @returns Returns the Reading or null if the response wan't ok
     */
    private async FetchLidarReading(): Promise<LidarReading | null>
    {
        let endpoint = "/api/data";

        let data: LidarReading;

        if (typeof fetch === "function")
        {
            let response = await fetch(endpoint);

            if (response.ok == false)
            {
                console.error("Error while fetching lidar reading: Status %i - %s", response.status, response.statusText);
                throw null;
            }

            data = await response.json();
        }
        else
        {
            data = await new Promise((resolve, error) =>
            {
                let xhttp = new XMLHttpRequest();
                xhttp.onreadystatechange = function() {
                    if (this.readyState == 4 && this.status == 200) {
                        resolve(JSON.parse(xhttp.responseText));
                    }
                    else
                    {
                        console.error("Error while fetching lidar reading: Status %i - %s", this.status, xhttp.statusText);
                        error(null);
                    }
                };
                xhttp.open("GET", "filename", true);
                xhttp.send();
            });
        }        

        return data;
    }
    // #endregion
}

function GetElementOrThrowError<TElement extends Element>(selector: string, name: string): TElement
{
    let element = document.querySelector<TElement>(selector);

    if (element == null)
    {
        throw new Error(`The ${name} "${selector} was not found."`);
    }

    return element;
}

let loadingProgressQuery = ".load-process";

let overlayContainer = GetElementOrThrowError<HTMLElement>(".Overlay", "OverlayContainer");
let progressElement = GetElementOrThrowError<HTMLElement>(loadingProgressQuery, "Progress");
let loadingTextElement = GetElementOrThrowError<HTMLElement>(".load-label-percent", "LoadingText");
// let volumeTextElement = GetElementOrThrowError<HTMLElement>(".StatsBox.Volume .Value", "Volume-Text");
let maxVolumeTextElement = GetElementOrThrowError<HTMLElement>(".StatsBox.Volume .MaxValue", "MaxVolumeTextElement");
let temperatureTextElement = GetElementOrThrowError<HTMLElement>(".StatsBox.Temperature .Value", "TemperatureText");
let distanceToFloorTextElement = GetElementOrThrowError<HTMLElement>(".StatsBox.Temperature .DistanceToFloor", "DistanceToFloorTextElement");
let distanceToCeilingTextElement = GetElementOrThrowError<HTMLElement>(".StatsBox.Temperature .DistanceToCeiling", "DistanceToCeilingTextElement");

if (progressElement instanceof HTMLProgressElement == false)
{
    console.warn(`The Progress-Element "${loadingProgressQuery} wasn't a HTMLProgressElement. Might not work..."`);
}

let mainHandler = new MainHandler(window, ".CanvasContainer", "Main", "Lidar");
(<any>window).MainHandler = mainHandler;

let numberFormatterNoDecimals = new Intl.NumberFormat(
    undefined,
    {
        minimumFractionDigits: 0,
        maximumFractionDigits: 0
    }
);
let numberFormatterLong = new Intl.NumberFormat(
    undefined,
    {
        minimumFractionDigits: 2,
        maximumFractionDigits: 2
    }
);

mainHandler.AddGlbLoadingprogessEventHandler((loadingProgess) => {
    if (loadingProgess < 100) {
        overlayContainer.classList.remove("Hide");
    } else {
        overlayContainer.classList.add("Hide");
    }

    progressElement.setAttribute("value", loadingProgess.toString());
    loadingTextElement.innerText = loadingProgess.toString();
});

mainHandler.AddGlbLoadedEventHandler(()  => {
    maxVolumeTextElement.innerText = numberFormatterLong.format(mainHandler.CalculateMainContainerVolume());
});

mainHandler.AddNewDataEventHandler(()  => {
    if (mainHandler.LastSensorTemperature == null)
    {
        temperatureTextElement.innerHTML = "error";
    }
    else
    {
        temperatureTextElement.innerText = numberFormatterNoDecimals.format(mainHandler.LastSensorTemperature);
    }

    if (mainHandler.LastDistanceToContainer?.FromFloor == null)
    {
        distanceToFloorTextElement.innerHTML = "error";
    }
    else
    {
        distanceToFloorTextElement.innerText = numberFormatterLong.format(mainHandler.LastDistanceToContainer.FromFloor);
    }

    if (mainHandler.LastDistanceToContainer?.FromCeiling == null)
    {
        distanceToCeilingTextElement.innerHTML = "error";
    }
    else
    {
        distanceToCeilingTextElement.innerText = numberFormatterLong.format(mainHandler.LastDistanceToContainer.FromCeiling);
    }
});

mainHandler.InitializeRender();