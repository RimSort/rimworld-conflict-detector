using HarmonyLib;

namespace TestPatches
{
    /// <summary>
    /// Patch with priority, before, and after ordering attributes.
    /// [HarmonyPriority(Priority.High)] — Priority.High is a const int
    /// [HarmonyBefore("other.mod")] — params string[] of mod IDs to run before
    /// [HarmonyAfter("dependency.mod")] — params string[] of mod IDs to run after
    /// </summary>
    [HarmonyPatch(typeof(System.String), "IsNullOrEmpty")]
    [HarmonyBefore("other.mod")]
    [HarmonyAfter("dependency.mod")]
    [HarmonyPriority(Priority.High)]
    public static class PriorityPatch
    {
        [HarmonyPrefix]
        public static void Prefix() { }
    }
}
